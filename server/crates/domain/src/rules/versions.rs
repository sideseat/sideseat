//! Release versions, compared the way a package's own scheme orders them.
//!
//! A version gate is the last resort of the rule language (a shape test is preferred), so the comparison it
//! rests on is written out here rather than borrowed: PEP 440 for Python packages and Semantic Versioning 2.0.0
//! for the rest. A value that does not parse is not a version - the gate is then unknown, never "the latest".

use std::cmp::Ordering;

/// How a package numbers its releases.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum VersionScheme {
    /// Python's PEP 440: epochs, any number of release segments, `aN`/`bN`/`rcN`, `.postN`, `.devN`, `+local`.
    Pep440,
    /// Semantic Versioning 2.0.0: `MAJOR.MINOR.PATCH`, `-prerelease`, `+build`.
    Semver,
}

/// One parsed version, ordered as its scheme orders releases.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Version {
    Pep440(Pep440),
    Semver(Semver),
}

impl Version {
    /// A version as a package reports it. `None` where the text is not one in this scheme.
    pub fn parse(scheme: VersionScheme, text: &str) -> Option<Self> {
        match scheme {
            VersionScheme::Pep440 => Pep440::parse(text).map(Self::Pep440),
            VersionScheme::Semver => Semver::parse(text, false).map(Self::Semver),
        }
    }

    /// A bound as an asset writes it. SemVer bounds may leave minor and patch out (`"6"` is `6.0.0`), because
    /// a range over major releases is the common case and spelling `6.0.0` adds nothing; values may not.
    pub fn parse_bound(scheme: VersionScheme, text: &str) -> Option<Self> {
        match scheme {
            VersionScheme::Pep440 => Pep440::parse(text).map(Self::Pep440),
            VersionScheme::Semver => Semver::parse(text, true).map(Self::Semver),
        }
    }

    /// Two versions of one scheme in release order; `None` across schemes.
    pub fn compare(&self, other: &Self) -> Option<Ordering> {
        match (self, other) {
            (Self::Pep440(a), Self::Pep440(b)) => Some(a.cmp(b)),
            (Self::Semver(a), Self::Semver(b)) => Some(a.cmp(b)),
            _ => None,
        }
    }
}

// ----------------------------------------------------------------------------
// PEP 440

/// A PEP 440 version, normalised: case-insensitive, `alpha`/`beta`/`c`/`pre`/`preview` spellings, `-`/`_`/`.`
/// separators, and an implicit `0` where a number is left out (`1.0a` is `1.0a0`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pep440 {
    epoch: u64,
    /// Without trailing zeros: `1.0` and `1` are one release.
    release: Vec<u64>,
    /// `(phase, n)`: `a` is 0, `b` 1, `rc` 2.
    pre: Option<(u8, u64)>,
    post: Option<u64>,
    dev: Option<u64>,
    local: Vec<LocalSegment>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum LocalSegment {
    Number(u64),
    Text(String),
}

impl Ord for LocalSegment {
    // A numeric segment sorts after a textual one, and numbers compare as numbers.
    fn cmp(&self, other: &Self) -> Ordering {
        match (self, other) {
            (Self::Number(a), Self::Number(b)) => a.cmp(b),
            (Self::Text(a), Self::Text(b)) => a.cmp(b),
            (Self::Number(_), Self::Text(_)) => Ordering::Greater,
            (Self::Text(_), Self::Number(_)) => Ordering::Less,
        }
    }
}

impl PartialOrd for LocalSegment {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Pep440 {
    pub fn parse(text: &str) -> Option<Self> {
        let lower = text.trim().to_ascii_lowercase();
        let text = lower.strip_prefix('v').unwrap_or(&lower);
        let (public, local) = match text.split_once('+') {
            Some((public, local)) => (public, Some(local)),
            None => (text, None),
        };
        let local = match local {
            None => Vec::new(),
            Some(local) => local
                .split(['.', '-', '_'])
                .map(|segment| {
                    if segment.is_empty() || !segment.bytes().all(|b| b.is_ascii_alphanumeric()) {
                        None
                    } else if segment.bytes().all(|b| b.is_ascii_digit()) {
                        segment.parse().ok().map(LocalSegment::Number)
                    } else {
                        Some(LocalSegment::Text(segment.to_string()))
                    }
                })
                .collect::<Option<Vec<_>>>()?,
        };
        let mut rest = public;
        let mut epoch = 0;
        if let Some((head, tail)) = rest.split_once('!') {
            epoch = number(head)?;
            rest = tail;
        }
        // The release: numbers joined by dots, and a dot only where a number follows it.
        let bytes = rest.as_bytes();
        let mut end = 0;
        while end < bytes.len()
            && (bytes[end].is_ascii_digit()
                || (bytes[end] == b'.' && bytes.get(end + 1).is_some_and(u8::is_ascii_digit)))
        {
            end += 1;
        }
        let (release_text, mut rest) = rest.split_at(end);
        let mut release: Vec<u64> = release_text.split('.').map(number).collect::<Option<_>>()?;
        while release.len() > 1 && release.last() == Some(&0) {
            release.pop();
        }
        let separator = |text: &str| {
            text.strip_prefix(['.', '-', '_'])
                .unwrap_or(text)
                .to_string()
        };
        let mut pre = None;
        let mut post = None;
        let mut dev = None;
        let after_sep = separator(rest);
        if let Some((phase, after)) = [
            ("alpha", 0),
            ("beta", 1),
            ("preview", 2),
            ("pre", 2),
            ("rc", 2),
            ("a", 0),
            ("b", 1),
            ("c", 2),
        ]
        .iter()
        .find_map(|(word, phase)| {
            after_sep
                .strip_prefix(word)
                .map(|after| (*phase, after.to_string()))
        }) {
            let (n, after) = suffix_number(&after);
            pre = Some((phase, n));
            rest = &public[public.len() - after.len()..];
        }
        let after_sep = separator(rest);
        if let Some(after) = ["post", "rev", "r"]
            .iter()
            .find_map(|word| after_sep.strip_prefix(word))
        {
            let (n, after) = suffix_number(after);
            post = Some(n);
            rest = &public[public.len() - after.len()..];
        } else if let Some(digits) = rest
            .strip_prefix('-')
            .filter(|d| d.starts_with(|c: char| c.is_ascii_digit()))
        {
            // The implicit post-release, `1.0-1`: a hyphen and a number alone.
            let (n, after) = suffix_number(digits);
            post = Some(n);
            rest = &public[public.len() - after.len()..];
        }
        let after_sep = separator(rest);
        if let Some(after) = after_sep.strip_prefix("dev") {
            let (n, after) = suffix_number(after);
            dev = Some(n);
            rest = &public[public.len() - after.len()..];
        }
        if !rest.is_empty() {
            return None;
        }
        Some(Self {
            epoch,
            release,
            pre,
            post,
            dev,
            local,
        })
    }

    /// The key a release sorts by, after epoch and release: a dev-only release sorts before every pre-release
    /// of it, pre-releases before the final release, and post-releases after it.
    fn phase_key(&self) -> (i8, u64, i8, u64, i8, u64) {
        let (pre_phase, pre_n) = match (self.pre, self.post, self.dev) {
            (Some((phase, n)), _, _) => (phase as i8, n),
            // `1.0.dev0`, with no pre and no post, is before `1.0a0`.
            (None, None, Some(_)) => (-1, 0),
            (None, _, _) => (3, 0),
        };
        let (post_flag, post_n) = match self.post {
            Some(n) => (1, n),
            None => (0, 0),
        };
        let (dev_flag, dev_n) = match self.dev {
            Some(n) => (0, n),
            None => (1, 0),
        };
        (pre_phase, pre_n, post_flag, post_n, dev_flag, dev_n)
    }
}

impl Ord for Pep440 {
    fn cmp(&self, other: &Self) -> Ordering {
        let release = |v: &Self, i: usize| v.release.get(i).copied().unwrap_or(0);
        let length = self.release.len().max(other.release.len());
        self.epoch
            .cmp(&other.epoch)
            .then_with(|| {
                (0..length)
                    .map(|i| release(self, i).cmp(&release(other, i)))
                    .find(|ordering| ordering.is_ne())
                    .unwrap_or(Ordering::Equal)
            })
            .then_with(|| self.phase_key().cmp(&other.phase_key()))
            // A local version sorts after the public one it extends.
            .then_with(|| self.local.cmp(&other.local))
    }
}

impl PartialOrd for Pep440 {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl std::fmt::Display for Pep440 {
    /// The normalised spelling: `1!2.0a1.post3.dev4+local.5`.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.epoch != 0 {
            write!(f, "{}!", self.epoch)?;
        }
        let release: Vec<String> = self.release.iter().map(u64::to_string).collect();
        write!(f, "{}", release.join("."))?;
        if let Some((phase, n)) = self.pre {
            write!(f, "{}{n}", ["a", "b", "rc"][usize::from(phase)])?;
        }
        if let Some(n) = self.post {
            write!(f, ".post{n}")?;
        }
        if let Some(n) = self.dev {
            write!(f, ".dev{n}")?;
        }
        if !self.local.is_empty() {
            let local: Vec<String> = self
                .local
                .iter()
                .map(|segment| match segment {
                    LocalSegment::Number(n) => n.to_string(),
                    LocalSegment::Text(text) => text.clone(),
                })
                .collect();
            write!(f, "+{}", local.join("."))?;
        }
        Ok(())
    }
}

fn number(text: &str) -> Option<u64> {
    (!text.is_empty() && text.bytes().all(|b| b.is_ascii_digit()))
        .then(|| text.parse().ok())
        .flatten()
}

/// The number after a suffix word, optionally after a separator; an omitted number is `0`.
fn suffix_number(text: &str) -> (u64, &str) {
    let trimmed = text.strip_prefix(['.', '-', '_']).unwrap_or(text);
    let digits = trimmed.bytes().take_while(u8::is_ascii_digit).count();
    if digits == 0 {
        return (0, text);
    }
    (
        trimmed[..digits].parse().unwrap_or(u64::MAX),
        &trimmed[digits..],
    )
}

// ----------------------------------------------------------------------------
// Semantic Versioning 2.0.0

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Semver {
    core: [u64; 3],
    pre: Vec<Identifier>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Identifier {
    Number(u64),
    Text(String),
}

impl Ord for Identifier {
    // Numeric identifiers sort before alphanumeric ones, numbers as numbers, text in ASCII order.
    fn cmp(&self, other: &Self) -> Ordering {
        match (self, other) {
            (Self::Number(a), Self::Number(b)) => a.cmp(b),
            (Self::Text(a), Self::Text(b)) => a.cmp(b),
            (Self::Number(_), Self::Text(_)) => Ordering::Less,
            (Self::Text(_), Self::Number(_)) => Ordering::Greater,
        }
    }
}

impl PartialOrd for Identifier {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Semver {
    /// `partial` admits `6` and `6.1` (missing parts are `0`), for bounds.
    pub fn parse(text: &str, partial: bool) -> Option<Self> {
        let text = text.trim();
        let text = text.strip_prefix('v').unwrap_or(text);
        // Build metadata never takes part in precedence.
        let text = text.split_once('+').map_or(
            text,
            |(version, build)| {
                if build.is_empty() { "" } else { version }
            },
        );
        let (core_text, pre_text) = match text.split_once('-') {
            Some((core, pre)) => (core, Some(pre)),
            None => (text, None),
        };
        let parts: Vec<&str> = core_text.split('.').collect();
        if parts.len() > 3 || (!partial && parts.len() != 3) {
            return None;
        }
        let mut core = [0u64; 3];
        for (slot, part) in core.iter_mut().zip(&parts) {
            // No leading zeros, as the specification requires of numeric parts.
            if part.len() > 1 && part.starts_with('0') {
                return None;
            }
            *slot = number(part)?;
        }
        let pre = match pre_text {
            None => Vec::new(),
            Some(pre) => pre
                .split('.')
                .map(|identifier| {
                    if identifier.is_empty()
                        || !identifier
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
                    {
                        None
                    } else if identifier.bytes().all(|b| b.is_ascii_digit()) {
                        (identifier.len() == 1 || !identifier.starts_with('0'))
                            .then(|| identifier.parse().ok().map(Identifier::Number))
                            .flatten()
                    } else {
                        Some(Identifier::Text(identifier.to_string()))
                    }
                })
                .collect::<Option<Vec<_>>>()?,
        };
        Some(Self { core, pre })
    }
}

impl std::fmt::Display for Semver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.core[0], self.core[1], self.core[2])?;
        if !self.pre.is_empty() {
            let pre: Vec<String> = self
                .pre
                .iter()
                .map(|identifier| match identifier {
                    Identifier::Number(n) => n.to_string(),
                    Identifier::Text(text) => text.clone(),
                })
                .collect();
            write!(f, "-{}", pre.join("."))?;
        }
        Ok(())
    }
}

impl Ord for Semver {
    fn cmp(&self, other: &Self) -> Ordering {
        self.core
            .cmp(&other.core)
            .then_with(|| match (self.pre.is_empty(), other.pre.is_empty()) {
                // A pre-release sorts before its release.
                (true, true) => Ordering::Equal,
                (true, false) => Ordering::Greater,
                (false, true) => Ordering::Less,
                (false, false) => self.pre.cmp(&other.pre),
            })
    }
}

impl PartialOrd for Semver {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pep(text: &str) -> Pep440 {
        Pep440::parse(text).unwrap_or_else(|| panic!("`{text}` is a PEP 440 version"))
    }

    fn sem(text: &str) -> Semver {
        Semver::parse(text, false).unwrap_or_else(|| panic!("`{text}` is a SemVer version"))
    }

    /// The ordering PEP 440 gives as its own example, in its order.
    #[test]
    fn pep440_orders_its_specification_example() {
        let ordered = [
            "1.dev0",
            "1.0.dev456",
            "1.0a1",
            "1.0a2.dev456",
            "1.0a12.dev456",
            "1.0a12",
            "1.0b1.dev456",
            "1.0b2",
            "1.0b2.post345.dev456",
            "1.0b2.post345",
            "1.0rc1.dev456",
            "1.0rc1",
            "1.0",
            "1.0+abc.5",
            "1.0+abc.7",
            "1.0+5",
            "1.0.post456.dev34",
            "1.0.post456",
            "1.0.15",
            "1.1.dev1",
        ];
        for pair in ordered.windows(2) {
            assert_eq!(
                pep(pair[0]).cmp(&pep(pair[1])),
                Ordering::Less,
                "{} < {}",
                pair[0],
                pair[1]
            );
        }
    }

    #[test]
    fn pep440_normalises_spellings() {
        for (a, b) in [
            ("1.0", "1"),
            ("1.0.0", "1"),
            ("v1.2", "1.2"),
            ("1.0ALPHA1", "1.0a1"),
            ("1.0-beta.2", "1.0b2"),
            ("1.0c1", "1.0rc1"),
            ("1.0pre", "1.0rc0"),
            ("1.0.post1", "1.0post1"),
            ("1.0-1", "1.0.post1"),
            ("1.0-dev2", "1.0.dev2"),
            ("1!1.0", "1!1"),
        ] {
            assert_eq!(pep(a).cmp(&pep(b)), Ordering::Equal, "{a} == {b}");
        }
        assert!(pep("1!0.1") > pep("2.0"), "an epoch outranks every release");
        for malformed in ["", "x", "1..0", "1.0x", "1.0+", "1.0+a..b", "abc1.0"] {
            assert!(
                Pep440::parse(malformed).is_none(),
                "`{malformed}` is not a version"
            );
        }
    }

    /// SemVer's precedence example, in its order.
    #[test]
    fn semver_orders_its_specification_example() {
        let ordered = [
            "1.0.0-alpha",
            "1.0.0-alpha.1",
            "1.0.0-alpha.beta",
            "1.0.0-beta",
            "1.0.0-beta.2",
            "1.0.0-beta.11",
            "1.0.0-rc.1",
            "1.0.0",
            "1.0.1",
            "1.1.0",
            "2.0.0",
        ];
        for pair in ordered.windows(2) {
            assert_eq!(
                sem(pair[0]).cmp(&sem(pair[1])),
                Ordering::Less,
                "{} < {}",
                pair[0],
                pair[1]
            );
        }
        assert_eq!(
            sem("1.0.0+build.1").cmp(&sem("1.0.0+other")),
            Ordering::Equal,
            "build is ignored"
        );
        assert_eq!(sem("v2.3.4").cmp(&sem("2.3.4")), Ordering::Equal);
        for malformed in [
            "1.0",
            "1",
            "01.0.0",
            "1.0.0-",
            "1.0.0-01",
            "1.0.0-a..b",
            "1.0.0+",
            "x.y.z",
        ] {
            assert!(
                Semver::parse(malformed, false).is_none(),
                "`{malformed}` is not a version"
            );
        }
        assert_eq!(
            Semver::parse("6", true),
            Some(sem("6.0.0")),
            "a bound may leave parts out"
        );
        assert_eq!(Semver::parse("6.1", true), Some(sem("6.1.0")));
    }
}
