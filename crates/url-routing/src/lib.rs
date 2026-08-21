#![forbid(unsafe_code)]

//! Safe URL routing for Ratatoskr Extractor.

use std::fmt::Write as _;
use std::future::Future;
use std::net::IpAddr;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;

use sha2::{Digest as _, Sha256};
use url::Url;

/// Syntax and destination-port constraints applied before DNS.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoutingPolicy {
    /// Maximum accepted input length in bytes.
    pub max_url_length: usize,
    /// Destination ports that can be resolved and connected.
    pub allowed_ports: Vec<u16>,
}

/// Original and canonical forms of one accepted URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizedUrl {
    original: String,
    normalized: Url,
    routing_fingerprint: String,
}

/// Ownership-aware route chosen before generic fetching.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceRoute {
    /// GitHub-owned content service.
    GitHub,
    /// X-owned content service.
    X,
    /// Instagram-owned content service.
    Instagram,
    /// Threads-owned content service.
    Threads,
    /// Future public Reddit adapter.
    Reddit,
    /// Future public Hacker News adapter.
    HackerNews,
    /// Future transcript and media adapter.
    YouTube,
    /// Direct PDF candidate.
    Pdf,
    /// Generic safe web retrieval.
    GenericWeb,
}

/// Bounded classes for prohibited destination addresses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddressBlockClass {
    /// The address does not identify a host.
    Unspecified,
    /// The address targets this host.
    Loopback,
    /// The address targets a private network.
    Private,
    /// The address belongs to shared carrier space.
    Shared,
    /// The address is local to one network link.
    LinkLocal,
    /// The address is reserved for documentation.
    Documentation,
    /// The address is reserved for benchmarks.
    Benchmarking,
    /// The address targets a multicast group.
    Multicast,
    /// The address is reserved or transitional.
    Reserved,
    /// The address targets a cloud metadata endpoint.
    Metadata,
}

/// A destination address was rejected before transport use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("the destination address is prohibited ({class:?})")]
pub struct AddressPolicyError {
    /// Stable policy class safe for operator reports.
    pub class: AddressBlockClass,
}

/// Boxed asynchronous DNS lookup result.
pub type LookupFuture<'a> =
    Pin<Box<dyn Future<Output = Result<Vec<SocketAddr>, DnsLookupError>> + Send + 'a>>;

/// Small injectable DNS effect used by the validating resolver.
pub trait DnsLookup: Send + Sync {
    /// Resolves `host` to a complete answer set with port zero.
    fn lookup(&self, host: String) -> LookupFuture<'_>;
}

/// The underlying DNS transport failed.
#[derive(Debug, Clone, Copy, thiserror::Error)]
#[error("DNS resolution failed")]
pub struct DnsLookupError;

/// A complete DNS answer failed before transport use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ResolutionError {
    /// DNS transport failed.
    #[error("DNS resolution failed")]
    Dns,
    /// DNS succeeded without any destination.
    #[error("DNS returned no destinations")]
    Empty,
    /// At least one address violated network policy.
    #[error("DNS answer was denied by destination policy ({class:?})")]
    Policy {
        /// Stable class that does not contain the address.
        class: AddressBlockClass,
    },
}

/// Reqwest resolver that validates every complete DNS answer.
#[derive(Debug, Clone)]
pub struct ValidatingResolver<R> {
    lookup: Arc<R>,
}

/// Tokio system DNS lookup used by the production resolver.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemDnsLookup;

/// Why URL normalization failed before DNS.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum UrlError {
    /// The input exceeds the configured parser boundary.
    #[error("the URL exceeds the configured length limit")]
    TooLong,
    /// The input is not a URL.
    #[error("the URL is not syntactically valid")]
    Invalid(#[source] url::ParseError),
    /// Only HTTP and HTTPS retrieval is supported.
    #[error("the URL scheme is not allowed")]
    UnsupportedScheme,
    /// The URL has no destination host.
    #[error("the URL has no host")]
    MissingHost,
    /// Credentials in URL user information are forbidden.
    #[error("URL user information is not allowed")]
    UserInformation,
    /// Port zero cannot be connected.
    #[error("URL port zero is not allowed")]
    PortZero,
    /// The destination port is outside the configured allowlist.
    #[error("the URL port is not allowed")]
    PortDenied,
    /// A non-canonical numeric host form is ambiguous.
    #[error("the numeric host form is not allowed")]
    AmbiguousNumericHost,
}

impl Default for RoutingPolicy {
    fn default() -> Self {
        Self {
            max_url_length: 8_192,
            allowed_ports: vec![80, 443],
        }
    }
}

impl<R> ValidatingResolver<R>
where
    R: DnsLookup + 'static,
{
    /// Wraps one DNS lookup implementation with destination policy.
    #[must_use]
    pub fn new(lookup: R) -> Self {
        Self {
            lookup: Arc::new(lookup),
        }
    }

    /// Resolves a host into a complete answer set.
    ///
    /// # Errors
    ///
    /// Returns [`ResolutionError`] for DNS failure or an empty answer.
    pub async fn resolve_host(&self, host: &str) -> Result<Vec<SocketAddr>, ResolutionError> {
        let addresses = self
            .lookup
            .lookup(host.to_owned())
            .await
            .map_err(|_| ResolutionError::Dns)?;
        if addresses.is_empty() {
            Err(ResolutionError::Empty)
        } else {
            for address in &addresses {
                if let Err(error) = validate_address(address.ip()) {
                    return Err(ResolutionError::Policy { class: error.class });
                }
            }
            Ok(addresses)
        }
    }
}

impl DnsLookup for SystemDnsLookup {
    fn lookup(&self, host: String) -> LookupFuture<'_> {
        Box::pin(async move {
            tokio::net::lookup_host((host.as_str(), 0))
                .await
                .map(Iterator::collect)
                .map_err(|_| DnsLookupError)
        })
    }
}

impl<R> reqwest::dns::Resolve for ValidatingResolver<R>
where
    R: DnsLookup + 'static,
{
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let resolver = Self {
            lookup: Arc::clone(&self.lookup),
        };
        let host = name.as_str().to_owned();
        Box::pin(async move {
            let addresses = resolver
                .resolve_host(&host)
                .await
                .map_err(|error| Box::new(error) as Box<dyn std::error::Error + Send + Sync>)?;
            Ok(Box::new(addresses.into_iter()) as reqwest::dns::Addrs)
        })
    }
}

impl NormalizedUrl {
    /// Returns the exact caller-supplied URL.
    #[must_use]
    pub fn original(&self) -> &str {
        &self.original
    }

    /// Returns the deterministic routing URL.
    #[must_use]
    pub const fn normalized(&self) -> &Url {
        &self.normalized
    }

    /// Returns the SHA-256 fingerprint of the routing URL.
    #[must_use]
    pub fn routing_fingerprint(&self) -> &str {
        &self.routing_fingerprint
    }
}

/// Parses a URL into preserved original and canonical routing forms.
///
/// # Errors
///
/// Returns [`UrlError`] when parsing fails.
pub fn normalize(input: &str, policy: &RoutingPolicy) -> Result<NormalizedUrl, UrlError> {
    if input.len() > policy.max_url_length {
        return Err(UrlError::TooLong);
    }
    if !input.contains("://") {
        return Err(UrlError::MissingHost);
    }
    let mut normalized = Url::parse(input).map_err(UrlError::Invalid)?;
    if !matches!(normalized.scheme(), "http" | "https") {
        return Err(UrlError::UnsupportedScheme);
    }
    if normalized.host().is_none() {
        return Err(UrlError::MissingHost);
    }
    if !normalized.username().is_empty() || normalized.password().is_some() {
        return Err(UrlError::UserInformation);
    }
    if normalized.port() == Some(0) {
        return Err(UrlError::PortZero);
    }
    let port = normalized
        .port_or_known_default()
        .ok_or(UrlError::PortDenied)?;
    if !policy.allowed_ports.contains(&port) {
        return Err(UrlError::PortDenied);
    }
    if numeric_host_is_ambiguous(input, &normalized) {
        return Err(UrlError::AmbiguousNumericHost);
    }
    normalized.set_fragment(None);
    remove_tracking_query(&mut normalized);
    let routing_fingerprint = fingerprint(normalized.as_str());
    Ok(NormalizedUrl {
        original: input.to_owned(),
        normalized,
        routing_fingerprint,
    })
}

/// Classifies source ownership without network access.
#[must_use]
pub fn classify(url: &NormalizedUrl) -> SourceRoute {
    let Some(host) = url.normalized.host_str() else {
        return SourceRoute::GenericWeb;
    };
    if host_matches(host, "github.com") {
        SourceRoute::GitHub
    } else if host_matches(host, "x.com") || host_matches(host, "twitter.com") {
        SourceRoute::X
    } else if host_matches(host, "instagram.com") {
        SourceRoute::Instagram
    } else if host_matches(host, "threads.net") {
        SourceRoute::Threads
    } else if host_matches(host, "reddit.com") || host_matches(host, "redd.it") {
        SourceRoute::Reddit
    } else if host_matches(host, "news.ycombinator.com") {
        SourceRoute::HackerNews
    } else if host_matches(host, "youtube.com") || host_matches(host, "youtu.be") {
        SourceRoute::YouTube
    } else if url.normalized.path().to_ascii_lowercase().ends_with(".pdf") {
        SourceRoute::Pdf
    } else {
        SourceRoute::GenericWeb
    }
}

/// Refuses non-global addresses without retaining their text in the error.
///
/// # Errors
///
/// Returns [`AddressPolicyError`] for every prohibited address class.
pub fn validate_address(address: IpAddr) -> Result<(), AddressPolicyError> {
    let class = match address {
        IpAddr::V4(address) => blocked_ipv4(address),
        IpAddr::V6(address) => blocked_ipv6(address),
    };
    match class {
        Some(class) => Err(AddressPolicyError { class }),
        None => Ok(()),
    }
}

fn blocked_ipv4(address: std::net::Ipv4Addr) -> Option<AddressBlockClass> {
    let [first, second, third, fourth] = address.octets();
    if address.is_unspecified() {
        Some(AddressBlockClass::Unspecified)
    } else if address.is_loopback() {
        Some(AddressBlockClass::Loopback)
    } else if (first == 169 && second == 254 && third == 169 && fourth == 254)
        || (first == 169 && second == 254 && third == 170 && fourth == 2)
    {
        Some(AddressBlockClass::Metadata)
    } else if first == 10
        || (first == 172 && (16..=31).contains(&second))
        || (first == 192 && second == 168)
    {
        Some(AddressBlockClass::Private)
    } else if first == 100 && (64..=127).contains(&second) {
        Some(AddressBlockClass::Shared)
    } else if first == 169 && second == 254 {
        Some(AddressBlockClass::LinkLocal)
    } else if (first == 192 && second == 0 && third == 2)
        || (first == 198 && second == 51 && third == 100)
        || (first == 203 && second == 0 && third == 113)
    {
        Some(AddressBlockClass::Documentation)
    } else if first == 198 && matches!(second, 18 | 19) {
        Some(AddressBlockClass::Benchmarking)
    } else if (224..=239).contains(&first) {
        Some(AddressBlockClass::Multicast)
    } else if first == 0
        || first >= 240
        || (first == 192 && second == 0 && third == 0)
        || (first == 192 && second == 88 && third == 99)
    {
        Some(AddressBlockClass::Reserved)
    } else {
        None
    }
}

fn blocked_ipv6(address: std::net::Ipv6Addr) -> Option<AddressBlockClass> {
    if let Some(mapped) = address.to_ipv4_mapped() {
        return blocked_ipv4(mapped);
    }
    let [first, second, third, fourth, fifth, sixth, seventh, eighth] = address.segments();
    if address.is_unspecified() {
        Some(AddressBlockClass::Unspecified)
    } else if address.is_loopback() {
        Some(AddressBlockClass::Loopback)
    } else if first == 0xfd00
        && second == 0x0ec2
        && third == 0
        && fourth == 0
        && fifth == 0
        && sixth == 0
        && seventh == 0
        && eighth == 0x0254
    {
        Some(AddressBlockClass::Metadata)
    } else if first & 0xfe00 == 0xfc00 {
        Some(AddressBlockClass::Private)
    } else if first & 0xffc0 == 0xfe80 {
        Some(AddressBlockClass::LinkLocal)
    } else if first & 0xff00 == 0xff00 {
        Some(AddressBlockClass::Multicast)
    } else if (first == 0x2001 && second == 0x0db8) || first & 0xfff0 == 0x3ff0 {
        Some(AddressBlockClass::Documentation)
    } else if first == 0x2001 && second == 0x0002 && third == 0 {
        Some(AddressBlockClass::Benchmarking)
    } else if is_reserved_ipv6([first, second, third, fourth, fifth, sixth, seventh, eighth]) {
        Some(AddressBlockClass::Reserved)
    } else {
        None
    }
}

fn is_reserved_ipv6(segments: [u16; 8]) -> bool {
    let [first, second, third, fourth, fifth, sixth, _, _] = segments;
    let discard_only =
        first == 0x0100 && second == 0 && third == 0 && fourth == 0 && fifth == 0 && sixth == 0;
    let nat64_well_known = first == 0x0064
        && second == 0xff9b
        && third == 0
        && fourth == 0
        && fifth == 0
        && sixth == 0;
    let nat64_local = first == 0x0064 && second == 0xff9b && third == 1;
    let transition = (first == 0x2001
        && (second == 0 || second & 0xfff0 == 0x0010 || second & 0xfff0 == 0x0020))
        || first == 0x2002;
    let site_local = first & 0xffc0 == 0xfec0;
    let outside_global_unicast = first & 0xe000 != 0x2000;
    discard_only
        || nat64_well_known
        || nat64_local
        || transition
        || site_local
        || outside_global_unicast
}

fn host_matches(host: &str, base: &str) -> bool {
    host == base
        || host
            .strip_suffix(base)
            .is_some_and(|prefix| prefix.ends_with('.'))
}

fn numeric_host_is_ambiguous(input: &str, parsed: &Url) -> bool {
    let Some(url::Host::Ipv4(_)) = parsed.host() else {
        return false;
    };
    match (raw_host(input), parsed.host_str()) {
        (Some(raw), Some(canonical)) => !raw.eq_ignore_ascii_case(canonical),
        _ => true,
    }
}

fn raw_host(input: &str) -> Option<&str> {
    let (_, after_scheme) = input.split_once("://")?;
    let authority = after_scheme.split(['/', '?', '#']).next()?;
    let host_port = authority.rsplit('@').next()?;
    if host_port.starts_with('[') {
        return None;
    }
    host_port.split(':').next()
}

fn remove_tracking_query(url: &mut Url) {
    let Some(query) = url.query() else {
        return;
    };
    let retained = query
        .split('&')
        .filter(|pair| !is_tracking_pair(pair))
        .collect::<Vec<_>>()
        .join("&");
    if retained.is_empty() {
        url.set_query(None);
    } else {
        url.set_query(Some(&retained));
    }
}

fn is_tracking_pair(pair: &str) -> bool {
    let raw_key = pair.split_once('=').map_or(pair, |(key, _)| key);
    let key = url::form_urlencoded::parse(raw_key.as_bytes())
        .next()
        .map(|(key, _)| key);
    matches!(
        key.as_deref(),
        Some(
            "utm_source"
                | "utm_medium"
                | "utm_campaign"
                | "utm_term"
                | "utm_content"
                | "gclid"
                | "fbclid"
        )
    )
}

fn fingerprint(value: &str) -> String {
    let digest = Sha256::digest(value.as_bytes());
    let mut output = String::with_capacity(digest.len() * 2);
    for byte in digest {
        let _ = write!(output, "{byte:02x}");
    }
    output
}
