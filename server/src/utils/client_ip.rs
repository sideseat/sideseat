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

use ipnet::IpNet;

/// A parsed trusted-proxy list, from CIDR blocks or bare addresses.
#[derive(Debug, Clone, Default)]
pub struct TrustedProxies {
    nets: Vec<IpNet>,
}

impl TrustedProxies {
    /// Parse a configured list. An unparseable entry is reported and skipped rather than failing startup:
    /// the consequence is that its address is not trusted, which is the safe direction.
    pub fn parse(entries: &[String]) -> Self {
        let mut nets = Vec::new();
        for entry in entries {
            let trimmed = entry.trim();
            if trimmed.is_empty() {
                continue;
            }
            // A bare address is a /32 or /128.
            match IpNet::from_str(trimmed) {
                Ok(net) => nets.push(net),
                Err(_) => match IpAddr::from_str(trimmed) {
                    Ok(addr) => nets.push(IpNet::from(addr)),
                    Err(_) => tracing::warn!(
                        entry = %trimmed,
                        "Ignoring an unparseable trusted-proxy entry; its address will not be trusted"
                    ),
                },
            }
        }
        Self { nets }
    }

    pub fn is_empty(&self) -> bool {
        self.nets.is_empty()
    }

    fn contains(&self, addr: IpAddr) -> bool {
        self.nets.iter().any(|net| net.contains(&addr))
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
    // Nothing vouched for the header, so only the peer is a fact.
    if trusted.is_empty() || !trusted.contains(peer) {
        return Some(peer.to_string());
    }
    // The peer is a trusted proxy: walk the chain from the right, skipping hops we also trust. The first
    // untrusted address is the furthest one a trusted proxy actually vouched for.
    if let Some(list) = forwarded_for {
        for hop in list.split(',').rev() {
            let hop = hop.trim();
            if hop.is_empty() {
                continue;
            }
            // A port may be attached (`1.2.3.4:5678`, or `[::1]:5678`).
            let bare = hop
                .strip_prefix('[')
                .and_then(|rest| rest.split(']').next())
                .unwrap_or_else(|| hop.rsplit_once(':').map_or(hop, |(head, _)| head));
            if let Ok(addr) = IpAddr::from_str(bare)
                && !trusted.contains(addr)
            {
                return Some(addr.to_string());
            }
        }
    }
    // Every hop was trusted, or none was parseable: the proxy itself is the best available attribution.
    Some(peer.to_string())
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
        let trusted = TrustedProxies::parse(&["10.0.0.0/8".to_string()]);
        assert_eq!(
            attributable_ip(Some(ip("10.1.2.3")), Some("198.51.100.7"), &trusted),
            Some("198.51.100.7".to_string())
        );
    }

    /// The rightmost *untrusted* hop wins, so a client cannot prepend a value to impersonate another.
    #[test]
    fn the_rightmost_untrusted_hop_wins() {
        let trusted = TrustedProxies::parse(&["10.0.0.0/8".to_string()]);
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
        let trusted = TrustedProxies::parse(&["10.1.2.3".to_string()]);
        assert_eq!(
            attributable_ip(Some(ip("10.1.2.3")), Some("198.51.100.7:44321"), &trusted),
            Some("198.51.100.7".to_string())
        );
    }

    /// Every hop trusted, or none parseable: fall back to the proxy rather than to nothing.
    #[test]
    fn an_all_trusted_chain_falls_back_to_the_peer() {
        let trusted = TrustedProxies::parse(&["10.0.0.0/8".to_string()]);
        assert_eq!(
            attributable_ip(Some(ip("10.1.2.3")), Some("10.4.5.6"), &trusted),
            Some("10.1.2.3".to_string())
        );
        assert_eq!(
            attributable_ip(Some(ip("10.1.2.3")), Some("not-an-ip"), &trusted),
            Some("10.1.2.3".to_string())
        );
    }

    /// An unparseable configuration entry is skipped, not trusted.
    #[test]
    fn an_unparseable_entry_is_not_trusted() {
        let trusted = TrustedProxies::parse(&["nonsense".to_string(), "10.0.0.0/8".to_string()]);
        assert!(!trusted.is_empty());
        // The good entry still works; the bad one trusts nothing.
        assert_eq!(
            attributable_ip(Some(ip("10.1.2.3")), Some("198.51.100.7"), &trusted),
            Some("198.51.100.7".to_string())
        );
        assert_eq!(
            attributable_ip(Some(ip("203.0.113.1")), Some("198.51.100.7"), &trusted),
            Some("203.0.113.1".to_string()),
            "an untrusted peer's header is still ignored"
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
}
