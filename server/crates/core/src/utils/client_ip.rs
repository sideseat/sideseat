//! Which address a rate limiter may attribute a request to.
//!
//! This is the third attempt at the question, and the two failures are the reason it is now one shared,
//! tested function rather than a line at each call site:
//!
//! * **Peer address only.** Unspoofable, but behind a proxy the peer *is* the proxy - so a few dozen invalid
//!   keys from one attacker exhausted the bucket every legitimate exporter shared, rejecting them all before
//!   authentication. An unauthenticated denial of service, produced by a limiter that is only defence in
//!   depth (the real protection is key entropy).
//! * **Forwarded header, trusted unconditionally.** No DoS, but a direct attacker sets a fresh value per
//!   request and never exhausts a bucket at all, so the limiter does nothing. Worse, where two transports
//!   shared a bucket namespace, a spoofed value on one could exhaust a bucket belonging to a peer on the
//!   other.
//!
//! Neither trade is acceptable, and no amount of care at the call site fixes it: *whether the forwarded
//! address can be believed is a property of the deployment*, which only configuration can state. So a
//! forwarded address counts only when the immediate peer is a configured trusted proxy, and the client is
//! then the rightmost hop that is not itself trusted - the last address a trusted proxy vouched for.
//!
//! With no trusted proxies configured (the default) the peer is always used, which is correct for a direct
//! deployment and refuses to believe a header nobody vouched for.

use std::net::IpAddr;
use std::str::FromStr;

use ipnet::{IpNet, Ipv4Net};

/// A parsed trusted-proxy list, from CIDR blocks or bare addresses.
#[derive(Debug, Clone, Default)]
pub struct TrustedProxies {
    nets: Vec<IpNet>,
}

impl TrustedProxies {
    /// Parse a configured list, **refusing** an entry that is not an address or CIDR block.
    ///
    /// Skipping a bad entry was the first behaviour and it is wrong here, for the reason this whole module
    /// exists: an unparsed proxy is an *untrusted* proxy, so every client behind it collapses into that one
    /// address's bucket - the denial of service the trusted-proxy list was added to remove. The server starts,
    /// nothing fails, and the limiter silently does the harmful thing. A typo in security-relevant
    /// configuration has to be a refusal with the entry named, not a warning in a log nobody reads.
    pub fn parse(entries: &[String]) -> Result<Self, String> {
        let mut nets = Vec::new();
        for entry in entries {
            let trimmed = entry.trim();
            if trimmed.is_empty() {
                continue;
            }
            // A bare address is a /32 or /128.
            if let Ok(net) = IpNet::from_str(trimmed) {
                nets.push(canonical_net(net));
            } else if let Ok(addr) = IpAddr::from_str(trimmed) {
                nets.push(IpNet::from(canonical(addr)));
            } else {
                return Err(format!(
                    "rate_limit.trusted_proxies entry {trimmed:?} is neither an IP address nor a CIDR block. \
                     Leaving it unparsed would put every client behind that proxy into one rate-limit bucket, \
                     so it is refused rather than skipped."
                ));
            }
        }
        Ok(Self { nets })
    }

    pub fn is_empty(&self) -> bool {
        self.nets.is_empty()
    }

    fn contains(&self, addr: IpAddr) -> bool {
        // Canonicalised first. A reverse proxy on a dual-stack socket connects as `::ffff:10.0.0.5`, and
        // `IpNet::contains` compares address families - so an operator's `10.0.0.0/8` matched nothing, the
        // proxy was treated as untrusted, and every forwarded client address was ignored. That silently
        // turns per-client rate limiting into one bucket for the whole proxy.
        let addr = canonical(addr);
        self.nets.iter().any(|net| net.contains(&addr))
    }
}

/// An IPv4-mapped IPv6 address as its IPv4 form; anything else unchanged.
///
/// `::ffff:10.0.0.5` and `10.0.0.5` are the same host, and every comparison here has to agree about that -
/// otherwise whether a proxy is trusted depends on whether the listener happens to be dual-stack.
/// A network written in IPv4-mapped IPv6 form as its IPv4 equivalent; anything else unchanged.
///
/// Canonicalising only the *peer* was half a fix: an operator who writes the mapped form
/// (`::ffff:10.0.0.5`, or `::ffff:10.0.0.0/104`) then had the peer converted to IPv4 while the configured
/// network stayed IPv6, so it stopped matching - the same silent collapse into one rate-limit bucket, arrived
/// at from the opposite side. Both sides are canonicalised, so the two forms are interchangeable in
/// configuration.
fn canonical_net(net: IpNet) -> IpNet {
    match net {
        IpNet::V6(v6) if v6.prefix_len() >= 96 => match v6.addr().to_ipv4_mapped() {
            Some(v4) => Ipv4Net::new(v4, v6.prefix_len() - 96)
                .map(IpNet::V4)
                .unwrap_or(net),
            None => net,
        },
        other => other,
    }
}

fn canonical(addr: IpAddr) -> IpAddr {
    match addr {
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => IpAddr::V4(v4),
            None => addr,
        },
        other => other,
    }
}

/// The address to attribute a request to, given the peer and any forwarded-for header value.
///
/// Returns `None` only when there is no peer address and nothing may be believed - in which case a caller
/// must not silently skip limiting, because "unknown" is not "unlimited".
pub fn attributable_ip(
    peer: Option<IpAddr>,
    forwarded_for: Option<&str>,
    trusted: &TrustedProxies,
) -> Option<String> {
    let peer = peer?;
    // Canonical before it becomes a bucket key. `::ffff:203.0.113.9` and `203.0.113.9` are the same client,
    // and returning whichever form the socket happened to produce gave that client *two* buckets on a
    // dual-stack listener - twice the allowance, from nothing the client did.
    let key = |addr: IpAddr| Some(canonical(addr).to_string());

    // Nothing vouched for the header, so only the peer is a fact.
    if trusted.is_empty() || !trusted.contains(peer) {
        return key(peer);
    }
    // The peer is a trusted proxy: walk the chain from the right, skipping hops we also trust. The first
    // untrusted address is the furthest one a trusted proxy actually vouched for.
    if let Some(list) = forwarded_for {
        for hop in list.split(',').rev() {
            let hop = hop.trim();
            if hop.is_empty() {
                continue;
            }
            if let Some(addr) = parse_hop(hop)
                && !trusted.contains(addr)
            {
                return key(addr);
            }
        }
    }
    // Every hop was trusted, or none was parseable: the proxy itself is the best available attribution.
    key(peer)
}

/// One forwarded hop as an address, tolerating an attached port.
///
/// The whole hop is tried **first**, because a bare IPv6 address is full of colons: stripping "the port" from
/// `2001:db8::1` by splitting on the last colon yields `2001:db8:`, which parses as nothing - so the hop was
/// discarded and attribution fell back to the proxy, putting every client behind it in one bucket again. Only
/// once the hop is known not to be an address on its own is a port considered.
fn parse_hop(hop: &str) -> Option<IpAddr> {
    if let Ok(addr) = IpAddr::from_str(hop) {
        return Some(addr);
    }
    // `[::1]:5678` - the bracketed form, which is the only unambiguous way to write IPv6 with a port.
    if let Some(inner) = hop.strip_prefix('[').and_then(|r| r.split(']').next())
        && let Ok(addr) = IpAddr::from_str(inner)
    {
        return Some(addr);
    }
    // `1.2.3.4:5678` - a single colon, so splitting is unambiguous.
    if hop.matches(':').count() == 1
        && let Some((head, _)) = hop.rsplit_once(':')
        && let Ok(addr) = IpAddr::from_str(head)
    {
        return Some(addr);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    /// With no trusted proxies, only the peer counts - a forwarded header nobody vouched for is ignored.
    ///
    /// This is what stops a direct attacker rotating the header to get a fresh bucket every request, which
    /// made the limiter do nothing at all.
    #[test]
    fn an_unvouched_header_is_ignored() {
        let trusted = TrustedProxies::default();
        assert_eq!(
            attributable_ip(Some(ip("203.0.113.9")), Some("1.2.3.4"), &trusted),
            Some("203.0.113.9".to_string())
        );
    }

    /// A header from a trusted peer names the client, so each client gets its own bucket.
    ///
    /// This is what stops one attacker behind a proxy exhausting the bucket every other exporter shares.
    #[test]
    fn a_trusted_proxy_names_its_client() {
        let trusted = TrustedProxies::parse(&["10.0.0.0/8".to_string()]).unwrap();
        assert_eq!(
            attributable_ip(Some(ip("10.1.2.3")), Some("198.51.100.7"), &trusted),
            Some("198.51.100.7".to_string())
        );
    }

    /// The rightmost *untrusted* hop wins, so a client cannot prepend a value to impersonate another.
    #[test]
    fn the_rightmost_untrusted_hop_wins() {
        let trusted = TrustedProxies::parse(&["10.0.0.0/8".to_string()]).unwrap();
        // The client claimed `1.1.1.1`; the real client as seen by the trusted proxy is `198.51.100.7`.
        assert_eq!(
            attributable_ip(
                Some(ip("10.1.2.3")),
                Some("1.1.1.1, 198.51.100.7, 10.9.9.9"),
                &trusted
            ),
            Some("198.51.100.7".to_string())
        );
    }

    /// A port on a hop does not defeat parsing.
    #[test]
    fn a_hop_may_carry_a_port() {
        let trusted = TrustedProxies::parse(&["10.1.2.3".to_string()]).unwrap();
        assert_eq!(
            attributable_ip(Some(ip("10.1.2.3")), Some("198.51.100.7:44321"), &trusted),
            Some("198.51.100.7".to_string())
        );
    }

    /// A bare IPv6 hop is attributed to the client, not discarded.
    ///
    /// Splitting "the port" off `2001:db8::1` at the last colon produced `2001:db8:`, which parses as
    /// nothing - so the hop was dropped and attribution fell back to the proxy, collapsing every client
    /// behind it into one bucket. That is the denial of service the trusted-proxy work exists to remove.
    #[test]
    fn a_bare_ipv6_hop_is_attributed() {
        let trusted = TrustedProxies::parse(&["10.0.0.0/8".to_string()]).unwrap();
        assert_eq!(
            attributable_ip(Some(ip("10.1.2.3")), Some("2001:db8::1"), &trusted),
            Some("2001:db8::1".to_string())
        );
    }

    /// The bracketed form with a port works too.
    #[test]
    fn a_bracketed_ipv6_hop_with_a_port_is_attributed() {
        let trusted = TrustedProxies::parse(&["10.0.0.0/8".to_string()]).unwrap();
        assert_eq!(
            attributable_ip(Some(ip("10.1.2.3")), Some("[2001:db8::1]:44321"), &trusted),
            Some("2001:db8::1".to_string())
        );
    }

    /// An IPv6 trusted proxy is matched as such, so its chain is walked.
    #[test]
    fn an_ipv6_proxy_can_be_trusted() {
        let trusted = TrustedProxies::parse(&["2001:db8::/32".to_string()]).unwrap();
        assert_eq!(
            attributable_ip(Some(ip("2001:db8::99")), Some("198.51.100.7"), &trusted),
            Some("198.51.100.7".to_string())
        );
    }

    /// Every hop trusted, or none parseable: fall back to the proxy rather than to nothing.
    #[test]
    fn an_all_trusted_chain_falls_back_to_the_peer() {
        let trusted = TrustedProxies::parse(&["10.0.0.0/8".to_string()]).unwrap();
        assert_eq!(
            attributable_ip(Some(ip("10.1.2.3")), Some("10.4.5.6"), &trusted),
            Some("10.1.2.3".to_string())
        );
        assert_eq!(
            attributable_ip(Some(ip("10.1.2.3")), Some("not-an-ip"), &trusted),
            Some("10.1.2.3".to_string())
        );
    }

    /// An unparseable configuration entry is **refused**, naming itself.
    ///
    /// Skipping it starts the server and silently collapses every client behind that proxy into one bucket -
    /// the denial of service this list exists to prevent. A typo in security-relevant configuration is a
    /// refusal, not a log line.
    #[test]
    fn an_unparseable_entry_is_refused() {
        let err = TrustedProxies::parse(&["nonsense".to_string(), "10.0.0.0/8".to_string()])
            .expect_err("a bad entry must be refused");
        assert!(
            err.contains("nonsense"),
            "the message must name the entry: {err}"
        );
    }

    /// Addresses and CIDR blocks are both accepted, and whitespace is tolerated.
    #[test]
    fn addresses_and_cidrs_are_both_accepted() {
        let trusted = TrustedProxies::parse(&[
            " 10.0.0.0/8 ".to_string(),
            "192.168.1.1".to_string(),
            "2001:db8::/32".to_string(),
            String::new(),
        ])
        .expect("all valid");
        assert_eq!(
            attributable_ip(Some(ip("192.168.1.1")), Some("198.51.100.7"), &trusted),
            Some("198.51.100.7".to_string())
        );
    }

    /// No peer address means no attribution - the caller must decide, not silently skip limiting.
    #[test]
    fn no_peer_means_no_attribution() {
        assert_eq!(
            attributable_ip(None, Some("1.2.3.4"), &TrustedProxies::default()),
            None
        );
    }

    /// The two ways of writing the same host are interchangeable, on both sides of the comparison.
    ///
    /// A reverse proxy on a dual-stack socket connects as `::ffff:10.0.0.5`, and `IpNet::contains` compares
    /// address families - so an operator's `10.0.0.0/8` matched nothing, the proxy was untrusted, every
    /// forwarded client address was ignored, and per-client limiting became one bucket for the whole proxy.
    /// Canonicalising only the peer left the mirror image: an operator writing the *mapped* form had it stop
    /// matching an IPv4 peer.
    #[test]
    fn ipv4_mapped_and_plain_ipv4_are_the_same_host_either_way_round() {
        let plain = "10.0.0.5".parse::<IpAddr>().unwrap();
        let mapped = "::ffff:10.0.0.5".parse::<IpAddr>().unwrap();
        let forwarded = Some("203.0.113.9");

        // An IPv4 CIDR trusts a mapped peer.
        let v4_cidr = TrustedProxies::parse(&["10.0.0.0/8".to_string()]).unwrap();
        for peer in [plain, mapped] {
            assert_eq!(
                attributable_ip(Some(peer), forwarded, &v4_cidr),
                Some("203.0.113.9".to_string()),
                "peer {peer} is inside 10.0.0.0/8 however it is written"
            );
        }

        // And a mapped CIDR - or a mapped bare address - trusts a plain IPv4 peer.
        for entry in ["::ffff:10.0.0.0/104", "::ffff:10.0.0.5"] {
            let cidr = TrustedProxies::parse(&[entry.to_string()]).unwrap();
            for peer in [plain, mapped] {
                assert_eq!(
                    attributable_ip(Some(peer), forwarded, &cidr),
                    Some("203.0.113.9".to_string()),
                    "configured {entry} must trust peer {peer}"
                );
            }
        }
    }

    /// A host genuinely outside the range is still untrusted, so the canonicalisation has not widened it.
    #[test]
    fn canonicalisation_does_not_widen_a_trusted_range() {
        let cidr = TrustedProxies::parse(&["10.0.0.0/8".to_string()]).unwrap();
        for peer in ["192.0.2.7", "::ffff:192.0.2.7", "2001:db8::1"] {
            let addr = peer.parse::<IpAddr>().unwrap();
            assert_eq!(
                attributable_ip(Some(addr), Some("203.0.113.9"), &cidr),
                Some(canonical(addr).to_string()),
                "peer {peer} is not a trusted proxy, so the header must be ignored"
            );
        }
    }
}
