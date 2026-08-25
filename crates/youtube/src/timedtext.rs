//! Timed-text payload parsing for the XML and JSON3 wire formats.
//!
//! Both parsers are hand-bounded: byte budget first, then segment-count and per-segment text
//! budgets during collection. Hostile payloads terminate in typed errors instead of panics,
//! and every slice is taken through fallible `get` bounds rather than unchecked indexing.

use std::str::FromStr as _;

use serde::Deserialize;

use crate::YoutubeError;
use crate::YoutubeLimits;

/// One ordered transcript segment with millisecond timing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    /// Offset from video start in milliseconds.
    pub start_ms: u64,
    /// Segment duration in milliseconds.
    pub duration_ms: u64,
    /// Decoded plain text of the segment.
    pub text: String,
}

/// Parses one timed-text payload into ordered segments.
///
/// Both documented wire formats are accepted: the XML `<text start="" dur="">` shape and the
/// JSON3 `events` shape. The format is detected from the first non-whitespace byte.
///
/// # Errors
///
/// Returns [`YoutubeError`] when the payload exceeds its byte budget, carries more segments or
/// longer segment text than allowed, or matches neither schema.
pub fn parse_timedtext(bytes: &[u8], limits: &YoutubeLimits) -> Result<Vec<Segment>, YoutubeError> {
    if bytes.len() > limits.max_track_bytes {
        return Err(YoutubeError::ResourceLimit);
    }
    let payload = std::str::from_utf8(bytes).map_err(|_| YoutubeError::Schema)?;
    let trimmed = payload.trim_start();
    let Some(first) = trimmed.chars().next() else {
        return Err(YoutubeError::Schema);
    };
    let segments = match first {
        '<' => xml_segments(trimmed, limits)?,
        '{' => json3_segments(trimmed, limits)?,
        _ => return Err(YoutubeError::Schema),
    };
    if segments.is_empty() {
        return Err(YoutubeError::Schema);
    }
    Ok(segments)
}

fn xml_segments(payload: &str, limits: &YoutubeLimits) -> Result<Vec<Segment>, YoutubeError> {
    const NAMES: [&str; 2] = ["text", "p"];
    let mut segments = Vec::new();
    let mut cursor = payload;
    while let Some(open) = cursor.find('<') {
        let after_open = cursor.get(open + 1..).ok_or(YoutubeError::Schema)?;
        let name_end = after_open.find([' ', '>']).ok_or(YoutubeError::Schema)?;
        let name = after_open.get(..name_end).ok_or(YoutubeError::Schema)?;
        if !NAMES.contains(&name) {
            cursor = after_open;
            continue;
        }
        let tag_end = after_open.find('>').ok_or(YoutubeError::Schema)?;
        let attributes = after_open
            .get(name_end..tag_end)
            .ok_or(YoutubeError::Schema)?;
        let body_rest = after_open.get(tag_end + 1..).ok_or(YoutubeError::Schema)?;
        let closing = format!("</{name}>");
        let text_end = body_rest
            .find(closing.as_str())
            .ok_or(YoutubeError::Schema)?;
        let raw_text = body_rest.get(..text_end).ok_or(YoutubeError::Schema)?;
        cursor = body_rest
            .get(text_end + closing.len()..)
            .ok_or(YoutubeError::Schema)?;

        let timing = timing_from_attributes(attributes)?;
        let text = decode_entities(raw_text)?;
        collect_segment(&mut segments, timing, text, limits)?;
    }
    Ok(segments)
}

/// Extracts `(start_ms, duration_ms)` from `start`/`dur` float-second or `t`/`d`
/// integer-millisecond attributes; either pair form is accepted.
fn timing_from_attributes(attributes: &str) -> Result<(u64, u64), YoutubeError> {
    let start_attr = attribute_value(attributes, "start");
    let dur_attr = attribute_value(attributes, "dur");
    let tick_start_attr = attribute_value(attributes, "t");
    let tick_duration_attr = attribute_value(attributes, "d");
    if let (Some(start), Some(dur)) = (start_attr, dur_attr) {
        return Ok((seconds_to_millis(&start)?, seconds_to_millis(&dur)?));
    }
    if let (Some(tick), Some(span)) = (tick_start_attr, tick_duration_attr) {
        let start = u64::from_str(tick.trim()).map_err(|_| YoutubeError::Schema)?;
        let duration = u64::from_str(span.trim()).map_err(|_| YoutubeError::Schema)?;
        return Ok((start, duration));
    }
    Err(YoutubeError::Schema)
}

fn attribute_value(attributes: &str, name: &str) -> Option<String> {
    let mut search = attributes;
    while let Some(equals) = search.find('=') {
        let key = search.get(..equals)?.trim();
        let quoted = search.get(equals + 1..)?.strip_prefix('"')?;
        let close = quoted.find('"')?;
        let value = quoted.get(..close)?.to_owned();
        if key.ends_with(name) {
            return Some(value);
        }
        search = quoted.get(close + 1..)?;
    }
    None
}

fn seconds_to_millis(raw: &str) -> Result<u64, YoutubeError> {
    let seconds = f64::from_str(raw.trim()).map_err(|_| YoutubeError::Schema)?;
    if !seconds.is_finite() || !(0.0..9_223_372_036_854.0).contains(&seconds) {
        return Err(YoutubeError::Schema);
    }
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "the range check above bounds the value to whole non-negative milliseconds"
    )]
    {
        Ok((seconds * 1_000.0).round() as u64)
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Json3Payload {
    #[serde(default)]
    events: Vec<Json3Event>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Json3Event {
    #[serde(default)]
    t_start_ms: Option<i64>,
    #[serde(default)]
    d_duration_ms: Option<i64>,
    #[serde(default)]
    segs: Option<Vec<Json3Segment>>,
}

#[derive(Debug, Deserialize)]
struct Json3Segment {
    #[serde(default)]
    utf8: Option<String>,
}

fn json3_segments(payload: &str, limits: &YoutubeLimits) -> Result<Vec<Segment>, YoutubeError> {
    let parsed: Json3Payload = serde_json::from_str(payload).map_err(|_| YoutubeError::Schema)?;
    let mut segments = Vec::new();
    for event in parsed.events {
        let start = u64::try_from(non_negative(event.t_start_ms)?)
            .map_err(|_| YoutubeError::ResourceLimit)?;
        let duration = u64::try_from(non_negative(event.d_duration_ms)?)
            .map_err(|_| YoutubeError::ResourceLimit)?;
        let Some(parts) = event.segs else {
            continue;
        };
        let mut text = String::new();
        for part in parts {
            if let Some(fragment) = part.utf8 {
                text.push_str(&fragment);
            }
        }
        collect_segment(&mut segments, (start, duration), text, limits)?;
    }
    Ok(segments)
}

fn non_negative(value: Option<i64>) -> Result<i64, YoutubeError> {
    match value.unwrap_or(0) {
        negative if negative < 0 => Err(YoutubeError::Schema),
        fine => Ok(fine),
    }
}

fn collect_segment(
    segments: &mut Vec<Segment>,
    (start_ms, duration_ms): (u64, u64),
    text: String,
    limits: &YoutubeLimits,
) -> Result<(), YoutubeError> {
    if text.chars().count() > limits.max_segment_characters {
        return Err(YoutubeError::ResourceLimit);
    }
    if segments.len() >= limits.max_segments {
        return Err(YoutubeError::ResourceLimit);
    }
    segments.push(Segment {
        start_ms,
        duration_ms,
        text,
    });
    Ok(())
}

/// Decodes the timed-text entity repertoire; unrecognized entities pass through literally.
fn decode_entities(input: &str) -> Result<String, YoutubeError> {
    if !input.contains('&') {
        return Ok(input.to_owned());
    }
    let mut output = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(amp) = rest.find('&') {
        output.push_str(rest.get(..amp).ok_or(YoutubeError::Schema)?);
        let after = rest.get(amp..).ok_or(YoutubeError::Schema)?;
        let semicolon = after
            .char_indices()
            .take_while(|(index, _)| *index <= 10)
            .find(|(_, character)| *character == ';')
            .map(|(index, _)| index);
        let replaced = semicolon.and_then(|index| {
            let token = after.get(1..index)?;
            // Numeric entities carry a leading '#' ("&#39;", "&#x27;"); named ones do not.
            let replacement = if let Some(digits) = token
                .strip_prefix("#x")
                .or_else(|| token.strip_prefix("#X"))
            {
                u32::from_str_radix(digits, 16)
                    .ok()
                    .and_then(char::from_u32)
            } else if let Some(digits) = token.strip_prefix('#') {
                u32::from_str(digits).ok().and_then(char::from_u32)
            } else {
                match token {
                    "amp" => Some('&'),
                    "lt" => Some('<'),
                    "gt" => Some('>'),
                    "quot" => Some('"'),
                    "apos" => Some('\''),
                    _ => None,
                }
            }?;
            Some((replacement, index))
        });
        if let Some((character, index)) = replaced {
            output.push(character);
            rest = after.get(index + 1..).ok_or(YoutubeError::Schema)?;
        } else {
            output.push('&');
            rest = after.get(1..).ok_or(YoutubeError::Schema)?;
        }
    }
    output.push_str(rest);
    Ok(output)
}
