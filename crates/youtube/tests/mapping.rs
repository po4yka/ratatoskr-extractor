//! Video identity resolution tests across every documented `YouTube` URL form.

use extractor_youtube::IdentityError;
use extractor_youtube::resolve_identity;

const ID: &str = "dQw4w9WgXcQ";

#[test]
fn every_documented_form_resolves_to_one_identity_and_canonical_address() {
    let forms = [
        format!("https://www.youtube.com/watch?v={ID}"),
        format!("https://youtube.com/watch?v={ID}"),
        format!("https://m.youtube.com/watch?v={ID}"),
        format!("https://music.youtube.com/watch?v={ID}"),
        format!("https://youtu.be/{ID}"),
        format!("https://www.youtube.com/shorts/{ID}"),
        format!("https://www.youtube.com/live/{ID}"),
        format!("https://www.youtube.com/embed/{ID}"),
        format!("https://www.youtube.com/v/{ID}"),
        format!("https://www.youtube-nocookie.com/embed/{ID}"),
        format!("https://youtube-nocookie.com/watch?v={ID}"),
    ];
    let expected_canonical = format!("https://www.youtube.com/watch?v={ID}");

    for form in forms {
        let (identity, canonical) = resolve_identity(&form).expect("documented form resolves");
        assert_eq!(identity.as_str(), ID, "identity for {form}");
        assert_eq!(
            canonical.as_str(),
            expected_canonical,
            "canonical for {form}"
        );
    }
}

#[test]
fn share_attribution_parameters_never_enter_the_canonical_address() {
    let shared = format!("https://www.youtube.com/watch?v={ID}&si=share-token&t=93s");
    let (_, canonical) = resolve_identity(&shared).expect("shared watch URL resolves");
    assert_eq!(
        canonical.as_str(),
        format!("https://www.youtube.com/watch?v={ID}")
    );
}

#[test]
fn a_playlist_only_url_names_no_video() {
    let error = resolve_identity("https://www.youtube.com/playlist?list=PLsomeplaylist")
        .expect_err("playlist URL names no video");
    assert!(matches!(error, IdentityError::NotAVideo));
}

#[test]
fn a_watch_url_without_an_id_parameter_names_no_video() {
    let error = resolve_identity("https://www.youtube.com/watch?list=PLsomeplaylist")
        .expect_err("watch URL without v names no video");
    assert!(matches!(error, IdentityError::NotAVideo));
}

#[test]
fn an_oversized_or_undersized_candidate_id_is_malformed() {
    for url in [
        "https://youtu.be/tooshort".to_owned(),
        format!("https://youtu.be/{ID}0"),
        "https://www.youtube.com/watch?v=!!bad!id!11".to_owned(),
    ] {
        let error = resolve_identity(&url).expect_err("malformed ids are refused");
        assert!(matches!(error, IdentityError::MalformedId), "for {url}");
    }
}

#[test]
fn underscore_dash_and_alphanumeric_ids_are_accepted() {
    for id in ["abcdefghi_-", "__________A", "-----------"] {
        let (identity, _) =
            resolve_identity(&format!("https://youtu.be/{id}")).expect("alphanumeric id resolves");
        assert_eq!(identity.as_str(), id);
    }
}
