//! Video identity resolution across every documented `YouTube` URL form.

/// A validated eleven-character `YouTube` video identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoIdentity {
    video_id: String,
}

impl VideoIdentity {
    /// The validated video id string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.video_id
    }
}

/// The canonical watch address derived from one video identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanonicalWatchAddress {
    address: String,
}

impl CanonicalWatchAddress {
    /// The canonical `https://www.youtube.com/watch?v=<id>` URL string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.address
    }
}

/// Why a URL did not resolve to one video identity.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum IdentityError {
    /// The URL names no video identity (for example a playlist-only URL).
    #[error("URL does not name a YouTube video")]
    NotAVideo,
    /// A candidate id violates the eleven-character id alphabet.
    #[error("YouTube video id is malformed")]
    MalformedId,
}

/// Resolves any documented `YouTube` URL form to its video identity and canonical watch address.
///
/// # Errors
///
/// Returns [`IdentityError`] when the URL names no video or the id is malformed.
pub fn resolve_identity(
    url: &str,
) -> Result<(VideoIdentity, CanonicalWatchAddress), IdentityError> {
    let parsed = url::Url::parse(url).map_err(|_| IdentityError::NotAVideo)?;
    let host = parsed.host_str().ok_or(IdentityError::NotAVideo)?;
    let lowered = host.to_ascii_lowercase();
    let on_youtube_com = is_documented_host(&lowered, "youtube.com");
    let on_short_host = is_documented_host(&lowered, "youtu.be");
    let on_nocookie = is_documented_host(&lowered, "youtube-nocookie.com");
    if !on_youtube_com && !on_short_host && !on_nocookie {
        return Err(IdentityError::NotAVideo);
    }

    let candidate = if on_short_host && !on_nocookie {
        first_path_segment(&parsed)
    } else {
        path_candidate(&parsed)
    };
    let Some(candidate) = candidate else {
        return Err(IdentityError::NotAVideo);
    };
    let video_id = validated_id(&candidate)?;
    // The id alphabet excludes every character that would need escaping in a query value.
    let canonical = CanonicalWatchAddress {
        address: format!("https://www.youtube.com/watch?v={video_id}"),
    };
    Ok((VideoIdentity { video_id }, canonical))
}

/// Whether a lowered host is the documented host itself or a subdomain of it.
pub(crate) fn is_documented_host(host: &str, documented: &str) -> bool {
    host == documented
        || host
            .strip_suffix(documented)
            .is_some_and(|rest| rest.ends_with('.'))
}

fn first_path_segment(parsed: &url::Url) -> Option<String> {
    let mut segments = parsed.path().strip_prefix('/')?.split('/');
    segments.next().map(str::to_owned)
}

fn path_candidate(parsed: &url::Url) -> Option<String> {
    let segments: Vec<&str> = parsed.path().split('/').filter(|s| !s.is_empty()).collect();
    match segments.as_slice() {
        ["watch"] => parsed
            .query_pairs()
            .find(|(name, _)| name == "v")
            .map(|(_, value)| value.to_string()),
        ["shorts" | "live" | "embed" | "v", id] => Some((*id).to_owned()),
        _ => None,
    }
}

fn validated_id(candidate: &str) -> Result<String, IdentityError> {
    let well_formed = candidate.len() == 11
        && candidate
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if well_formed {
        Ok(candidate.to_owned())
    } else {
        Err(IdentityError::MalformedId)
    }
}
