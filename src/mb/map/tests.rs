//! Fixture tests of the MusicBrainz JSON mapping. The fixtures under
//! `tests/fixtures/musicbrainz/` are trimmed real web-service responses.

use serde_json::json;

use super::*;
use crate::settings::ScriptException;

const OK_COMPUTER: &str = include_str!("../../../tests/fixtures/musicbrainz/ok-computer.json");
const OK_COMPUTER_FULL: &str =
    include_str!("../../../tests/fixtures/musicbrainz/ok-computer-full.json");
const PLASTIC_BEACH_VINYL: &str =
    include_str!("../../../tests/fixtures/musicbrainz/plastic-beach-vinyl.json");
const PLASTIC_BEACH_CD_DVD: &str =
    include_str!("../../../tests/fixtures/musicbrainz/plastic-beach-cd-dvd.json");
const UTADA: &str = include_str!("../../../tests/fixtures/musicbrainz/utada-addicted-to-you.json");
const NOW_37: &str = include_str!("../../../tests/fixtures/musicbrainz/now-37-va.json");
const RECORDING_LOOKUP: &str =
    include_str!("../../../tests/fixtures/musicbrainz/recording-lookup.json");
const RELEASE_SEARCH: &str =
    include_str!("../../../tests/fixtures/musicbrainz/release-search.json");
const RECORDING_SEARCH: &str =
    include_str!("../../../tests/fixtures/musicbrainz/recording-search.json");

fn load(text: &str) -> Value {
    serde_json::from_str(text).expect("fixture is valid JSON")
}

fn with(mode: StandardizeArtistNames) -> MetadataSettings {
    MetadataSettings {
        standardize_artist_names: mode,
        ..MetadataSettings::default()
    }
}

fn map(text: &str, mode: StandardizeArtistNames) -> Release {
    release(&load(text), &with(mode), &[]).expect("fixture maps")
}

fn all(m: &Metadata, key: &str) -> Vec<String> {
    m.get_all(key).to_vec()
}

fn recording(rec: &Value, m: &mut Metadata, settings: &MetadataSettings) {
    recording_to_metadata(rec, m, None, None, settings, &mut TrackVotes::default());
}

#[test]
fn single_disc_release() {
    let release = map(OK_COMPUTER, StandardizeArtistNames::None);
    assert_eq!(
        release.id,
        ReleaseId {
            source: Source::MusicBrainz,
            id: "6a09041b-0f79-3278-88d9-0552dd28d37f".into()
        }
    );
    assert!(matches!(
        &release.artwork,
        Artwork::CoverArtArchive { release_group_id: Some(id), .. } if id == "b1392450-e666-3926-a536-22c65f834433"
    ));

    let album = &release.metadata;
    assert_eq!(album.get("album"), Some("OK Computer"));
    assert_eq!(album.get("albumartist"), Some("Radiohead"));
    assert_eq!(album.get("albumartistsort"), Some("Radiohead"));
    assert_eq!(
        all(album, "musicbrainz_albumartistid"),
        ["a74b1b7f-71a5-4011-9441-d0b5e4122711"]
    );
    assert_eq!(
        album.get("musicbrainz_albumid"),
        Some("6a09041b-0f79-3278-88d9-0552dd28d37f")
    );
    assert_eq!(
        album.get("musicbrainz_releasegroupid"),
        Some("b1392450-e666-3926-a536-22c65f834433")
    );
    assert_eq!(album.get("date"), Some("1997-07-01"));
    assert_eq!(
        album.get("originaldate"),
        Some("1997-05-21"),
        "release-group first-release-date"
    );
    assert_eq!(album.get("originalyear"), Some("1997"));
    assert_eq!(album.get("releasecountry"), Some("US"));
    assert_eq!(album.get("releasestatus"), Some("official"));
    assert_eq!(album.get("releasetype"), Some("album"));
    assert_eq!(album.get("barcode"), Some("724385522925"));
    assert_eq!(album.get("asin"), Some("B000002UJQ"));
    assert_eq!(album.get("script"), Some("Latn"));
    assert_eq!(album.get("~releaselanguage"), Some("eng"));
    assert!(
        !album.contains("language"),
        "the release language is not the lyrics language"
    );
    assert_eq!(album.get("label"), Some("Capitol Records"));
    assert_eq!(album.get("catalognumber"), Some("CDP 7243 8 55229 2 5"));
    assert_eq!(album.get("media"), Some("CD"));
    assert_eq!(album.get("totaldiscs"), Some("1"));
    assert_eq!(album.get("~totalalbumtracks"), Some("12"));
    assert!(
        !album.contains("tracknumber"),
        "album-level tags carry no per-track keys"
    );
    assert!(!album.contains("title"));
    // Release-level relationships apply to the whole album.
    assert_eq!(all(album, "producer"), ["Nigel Godrich", "Radiohead"]);
    assert_eq!(all(album, "conductor"), ["Nick Ingman"]);
    assert_eq!(
        all(album, "arranger"),
        ["Radiohead"],
        "instrument arranger maps to arranger"
    );

    assert_eq!(release.tracks.len(), 12);
    let t = &release.tracks[1].metadata;
    assert_eq!(release.tracks[1].id, "ccadc32f-698e-302e-bc5c-a5b8e5ba223e");
    assert_eq!(t.get("title"), Some("Paranoid Android"));
    assert_eq!(t.get("~recordingtitle"), Some("Paranoid Android"));
    assert_eq!(t.get("tracknumber"), Some("2"));
    assert_eq!(t.get("~musicbrainz_tracknumber"), Some("2"));
    assert_eq!(t.get("~absolutetracknumber"), Some("2"));
    assert_eq!(t.get("~totalalbumtracks"), Some("12"));
    assert_eq!(t.get("totaltracks"), Some("12"));
    assert_eq!(t.get("discnumber"), Some("1"));
    assert_eq!(t.get("totaldiscs"), Some("1"));
    assert_eq!(t.get("media"), Some("CD"));
    assert_eq!(t.get("artist"), Some("Radiohead"));
    assert_eq!(t.get("artistsort"), Some("Radiohead"));
    assert_eq!(all(t, "artists"), ["Radiohead"]);
    assert_eq!(all(t, "~artists_sort"), ["Radiohead"]);
    assert_eq!(
        all(t, "musicbrainz_artistid"),
        ["a74b1b7f-71a5-4011-9441-d0b5e4122711"]
    );
    assert_eq!(t.get("albumartist"), Some("Radiohead"));
    assert_eq!(t.get("album"), Some("OK Computer"));
    assert_eq!(
        t.get("musicbrainz_recordingid"),
        Some("9f9cf187-d6f9-437f-9d98-d59cdbd52757")
    );
    assert_eq!(
        t.get("musicbrainz_trackid"),
        Some("ccadc32f-698e-302e-bc5c-a5b8e5ba223e")
    );
    assert_eq!(all(t, "isrc"), ["GBAYE9701376", "GBBKS1700108"]);
    assert_eq!(
        t.length(),
        Some(Duration::from_millis(383_493)),
        "track length wins over recording length (384000)"
    );
    assert_eq!(release.tracks[0].metadata.get("title"), Some("Airbag"));
    assert_eq!(
        release.tracks[0].length(),
        Some(Duration::from_millis(284_400))
    );
    assert!(
        release
            .tracks
            .iter()
            .all(|t| !t.metadata.contains("~multiartist"))
    );
    assert!(
        release
            .tracks
            .iter()
            .all(|t| t.metadata.get("totaltracks") == Some("12"))
    );
    let numbers: Vec<_> = release
        .tracks
        .iter()
        .map(|t| t.metadata.get("tracknumber").unwrap_or("").to_owned())
        .collect();
    assert_eq!(numbers, (1..=12).map(|n| n.to_string()).collect::<Vec<_>>());
    let absolute: Vec<_> = release
        .tracks
        .iter()
        .map(|t| {
            t.metadata
                .get("~absolutetracknumber")
                .unwrap_or("")
                .to_owned()
        })
        .collect();
    assert_eq!(
        absolute,
        (1..=12).map(|n| n.to_string()).collect::<Vec<_>>()
    );
}

#[test]
fn multi_disc_release_with_different_artist_tracks() {
    let release = map(PLASTIC_BEACH_VINYL, StandardizeArtistNames::None);
    let album = &release.metadata;
    assert_eq!(album.get("totaldiscs"), Some("2"));
    assert_eq!(
        album.get("media"),
        Some("12\" Vinyl"),
        "identical formats listed once"
    );
    assert_eq!(album.get("albumartist"), Some("Gorillaz"));
    assert_eq!(album.get("label"), Some("Parlophone"));
    assert_eq!(album.get("catalognumber"), Some("626 1661"));
    assert_eq!(album.get("~totalalbumtracks"), Some("5"));

    let summary: Vec<_> = release
        .tracks
        .iter()
        .map(|t| {
            let m = &t.metadata;
            (
                m.get("discnumber").unwrap_or(""),
                m.get("tracknumber").unwrap_or(""),
                m.get("totaltracks").unwrap_or(""),
                m.get("title").unwrap_or(""),
                m.get("~absolutetracknumber").unwrap_or(""),
            )
        })
        .collect();
    assert_eq!(
        summary,
        [
            ("1", "1", "8", "Orchestral Intro", "1"),
            (
                "1",
                "2",
                "8",
                "Welcome to the World of the Plastic Beach",
                "2"
            ),
            ("1", "3", "8", "White Flag", "3"),
            ("2", "1", "8", "Some Kind of Nature", "4"),
            ("2", "2", "8", "On Melancholy Hill", "5"),
        ],
        "vinyl 'A01'/'C09' numbers are positions here, like Picard's tracknumber"
    );
    assert!(
        release
            .tracks
            .iter()
            .all(|t| t.metadata.get("totaldiscs") == Some("2"))
    );
    assert_eq!(
        release.tracks[0].metadata.get("~musicbrainz_tracknumber"),
        Some("A01")
    );

    // Track 1: same artist as the album; recording-level relationships mapped. The release has
    // several distinct track artists, so every track is `~multiartist` (Picard's rule).
    let intro = &release.tracks[0].metadata;
    assert_eq!(intro.get("artist"), Some("Gorillaz"));
    assert_eq!(intro.get("~multiartist"), Some("1"));
    assert_eq!(all(intro, "artists"), ["Gorillaz"]);
    assert_eq!(all(intro, "conductor"), ["André de Ridder"]);
    assert_eq!(
        all(intro, "arranger"),
        ["James Redwood", "André de Ridder"],
        "orchestrator maps to arranger"
    );
    assert_eq!(all(intro, "producer"), ["Gorillaz"]);
    assert_eq!(all(intro, "isrc"), ["GBAYE1000001", "GBAYE1400432"]);

    // Track 2: "Gorillaz & Snoop Dogg" — join phrase kept in the display string, one entry
    // per artist in the multi-valued tags. The track credit overrides the recording's.
    let welcome = &release.tracks[1].metadata;
    assert_eq!(welcome.get("artist"), Some("Gorillaz & Snoop Dogg"));
    assert_eq!(welcome.get("artistsort"), Some("Gorillaz & Snoop Dogg"));
    assert_eq!(all(welcome, "artists"), ["Gorillaz", "Snoop Dogg"]);
    assert_eq!(all(welcome, "~artists_sort"), ["Gorillaz", "Snoop Dogg"]);
    assert_eq!(
        all(welcome, "musicbrainz_artistid"),
        [
            "e21857d5-3256-4547-afb3-4b6ded592596",
            "f90e8b26-9e52-4669-a5c9-e28529c47894"
        ]
    );
    assert_eq!(all(welcome, "~artists_countries").len(), 2);
    assert_eq!(welcome.get("~multiartist"), Some("1"));
    assert_eq!(
        welcome.get("albumartist"),
        Some("Gorillaz"),
        "album artist is unaffected"
    );
    assert_eq!(all(welcome, "albumartists"), ["Gorillaz"]);
    assert_eq!(
        all(welcome, "musicbrainz_albumartistid"),
        ["e21857d5-3256-4547-afb3-4b6ded592596"]
    );

    // Track 3: three artists, different join phrases.
    let white_flag = &release.tracks[2].metadata;
    assert_eq!(white_flag.get("artist"), Some("Gorillaz, Bashy & Kano"));
    assert_eq!(all(white_flag, "artists"), ["Gorillaz", "Bashy", "Kano"]);
    assert_eq!(white_flag.get("~multiartist"), Some("1"));
    assert!(
        !white_flag.contains("~artistcomment"),
        "only for a single credited artist"
    );

    // Disc 2.
    let nature = &release.tracks[3].metadata;
    assert_eq!(nature.get("artist"), Some("Gorillaz & Lou Reed"));
    assert_eq!(nature.get("artistsort"), Some("Gorillaz & Reed, Lou"));
    assert_eq!(nature.get("~multiartist"), Some("1"));
    let hill = &release.tracks[4].metadata;
    assert_eq!(hill.get("artist"), Some("Gorillaz"));
    assert_eq!(
        hill.get("~multiartist"),
        Some("1"),
        "set on every track, not only on the different ones"
    );
    assert!(
        release
            .tracks
            .iter()
            .all(|t| t.metadata.get("~multiartist") == Some("1"))
    );
}

#[test]
fn multiartist_depends_on_distinct_track_artists_only() {
    // One artist throughout, even if it is not the album artist: not multi-artist.
    let mut va = load(OK_COMPUTER);
    va["artist-credit"][0]["name"] = json!("Various Artists");
    let mapped = release(&va, &MetadataSettings::default(), &[]).expect("maps");
    assert!(
        mapped
            .tracks
            .iter()
            .all(|t| !t.metadata.contains("~multiartist"))
    );

    // A single different track artist flags the whole album.
    let mut guest = load(OK_COMPUTER);
    guest["media"][0]["tracks"][3]["artist-credit"][0]["name"] = json!("Guest");
    let mapped = release(&guest, &with(StandardizeArtistNames::None), &[]).expect("maps");
    assert_eq!(mapped.tracks[3].metadata.get("artist"), Some("Guest"));
    assert_eq!(mapped.tracks.len(), 12);
    assert!(
        mapped
            .tracks
            .iter()
            .all(|t| t.metadata.get("~multiartist") == Some("1"))
    );
}

#[test]
fn per_medium_format_and_track_count_differ() {
    let release = map(PLASTIC_BEACH_CD_DVD, StandardizeArtistNames::None);
    assert_eq!(release.metadata.get("media"), Some("CD / DVD-Video"));
    assert_eq!(release.metadata.get("totaldiscs"), Some("2"));
    let cd = &release.tracks[0].metadata;
    assert_eq!(
        (cd.get("discnumber"), cd.get("media"), cd.get("totaltracks")),
        (Some("1"), Some("CD"), Some("16"))
    );
    let dvd = &release.tracks.last().expect("DVD track").metadata;
    assert_eq!(
        (
            dvd.get("discnumber"),
            dvd.get("media"),
            dvd.get("totaltracks")
        ),
        (Some("2"), Some("DVD-Video"), Some("1"))
    );
    assert_eq!(dvd.get("tracknumber"), Some("1"));
    assert_eq!(
        dvd.get("~absolutetracknumber"),
        Some(release.tracks.len().to_string().as_str()),
        "counts the tracks listed"
    );
}

#[test]
fn credited_vs_standardized_artist_names() {
    // Credit "Lou Reed" as "The Lou" on the track.
    let mut json = load(PLASTIC_BEACH_VINYL);
    json["media"][1]["tracks"][0]["artist-credit"][1]["name"] = json!("The Lou");
    let track = |json: &Value, mode| {
        release(json, &with(mode), &[]).expect("maps").tracks[3]
            .metadata
            .clone()
    };
    let credited = track(&json, StandardizeArtistNames::None);
    assert_eq!(credited.get("artist"), Some("Gorillaz & The Lou"));
    assert_eq!(all(&credited, "artists"), ["Gorillaz", "The Lou"]);
    for mode in [
        StandardizeArtistNames::All,
        StandardizeArtistNames::Variations,
    ] {
        // Without an ended alias "The Lou" is only a variation of the artist's name.
        let standardized = track(&json, mode);
        assert_eq!(
            standardized.get("artist"),
            Some("Gorillaz & Lou Reed"),
            "{mode:?}"
        );
        assert_eq!(all(&standardized, "artists"), ["Gorillaz", "Lou Reed"]);
        assert_eq!(standardized.get("artistsort"), Some("Gorillaz & Reed, Lou"));
    }
    // Variations keeps the credit when it is an ended alias (a real name change).
    json["media"][1]["tracks"][0]["artist-credit"][1]["artist"]["aliases"] = json!([
        {"name": "The Lou", "sort-name": "Lou, The", "type-id": null, "ended": true, "primary": null, "locale": null}
    ]);
    let renamed = track(&json, StandardizeArtistNames::Variations);
    assert_eq!(renamed.get("artist"), Some("Gorillaz & The Lou"));
    assert_eq!(renamed.get("artistsort"), Some("Gorillaz & Lou, The"));
    assert_eq!(
        track(&json, StandardizeArtistNames::All).get("artist"),
        Some("Gorillaz & Lou Reed")
    );
}

#[test]
fn work_level_relations_give_writers_and_languages() {
    let release = map(OK_COMPUTER, StandardizeArtistNames::None);
    let t = &release.tracks[1].metadata;
    assert_eq!(all(t, "language"), ["eng"]);
    assert_eq!(
        all(t, "writer"),
        [
            "Colin Greenwood",
            "Jonny Greenwood",
            "Ed O’Brien",
            "Philip Selway",
            "Thom Yorke"
        ],
        "writers come from the work's relationships"
    );
    assert_eq!(all(t, "~writersort").len(), 5);
    assert_eq!(
        t.get("musicbrainz_workid"),
        Some("1b4ff597-f43f-3dac-9f76-0e7b7f38d0d2")
    );
    assert_eq!(t.get("work"), Some("Paranoid Android"));
    // Release-level and recording-level credits of the same person collapse.
    assert_eq!(all(t, "conductor"), ["Nick Ingman"]);
    assert_eq!(all(t, "producer"), ["Nigel Godrich", "Radiohead"]);
    assert!(
        release.tracks[0].metadata.get("writer").is_none(),
        "no work relationships on track 1"
    );
}

#[test]
fn full_release_maps_hidden_variables_relationships_and_genres() {
    let mut settings = MetadataSettings {
        track_ars: true,
        ..MetadataSettings::default()
    };
    settings.genres.use_genres = true;
    let release = release(&load(OK_COMPUTER_FULL), &settings, &[]).expect("maps");
    let album = &release.metadata;
    assert_eq!(album.get("~releasegroup"), Some("OK Computer"));
    assert_eq!(
        album.get("~releasegroup_firstreleasedate"),
        Some("1997-05-21")
    );
    assert_eq!(album.get("~primaryreleasetype"), Some("album"));
    assert!(!album.contains("~secondaryreleasetype"));
    assert_eq!(album.get("~releasecountries"), Some("US"));
    assert_eq!(album.get("~releaselanguage"), Some("eng"));
    assert_eq!(album.get("asin"), Some("B000002UJQ"));
    assert_eq!(album.get("~albumartistcomment"), None);
    assert_eq!(all(album, "~albumartists"), ["Radiohead"]);
    assert_eq!(all(album, "~albumartists_countries"), ["GB"]);
    assert_eq!(
        album.get("~totalalbumtracks"),
        Some("3"),
        "the fixture is trimmed to three tracks"
    );
    assert_eq!(release.tracks.len(), 3);

    let t = &release.tracks[1].metadata;
    assert_eq!(t.get("title"), Some("Paranoid Android"));
    assert_eq!(t.get("~recording_firstreleasedate"), Some("1997-05-21"));
    assert_eq!(t.get("~recordingcomment"), None);
    // Performer relationships: instrument and vocal attributes.
    assert_eq!(all(t, "performer:percussion"), ["Colin Greenwood"]);
    assert_eq!(all(t, "performer:background vocals"), ["Ed O’Brien"]);
    assert_eq!(all(t, "arranger"), ["Radiohead"], "instrument arranger");
    assert_eq!(all(t, "mixer"), ["Nigel Godrich"]);
    // Series relationship of the recording.
    assert_eq!(
        t.get("~recording_series"),
        Some("Indie 88: Top 500 Indie Rock Songs")
    );
    assert_eq!(
        t.get("~recording_seriesid"),
        Some("33061f6b-6553-4d61-851b-71b790bf1460")
    );
    assert_eq!(t.get("~recording_seriesnumber"), Some("20"));
    assert!(
        !t.contains("~recording_seriescomment"),
        "empty comment is dropped"
    );
    // Work: ISWC and language.
    assert_eq!(t.get("~iswc"), Some("T-010.257.771-8"));
    assert_eq!(all(t, "language"), ["eng"]);
    // Disc ids of the medium.
    assert_eq!(t.get_all("~musicbrainz_discids").len(), 2);
    assert_eq!(album.get_all("~musicbrainz_discids").len(), 2);
    assert!(
        !t.contains("musicbrainz_discid"),
        "no disc id was looked up"
    );
    // Genres: release group + recording votes, default 90 % floor keeps only the top genre.
    assert_eq!(t.get_all("genre"), ["Alternative Rock"]);
    assert!(t.get_all("~genres").contains(&"art rock".to_owned()));
    assert!(!t.get_all("~folksonomy_tags").is_empty());
    assert!(!album.contains("genre"), "genres are computed per track");
}

#[test]
fn genre_settings_change_the_genre_tag() {
    let json = load(OK_COMPUTER_FULL);
    let genre = |f: &dyn Fn(&mut MetadataSettings)| {
        let mut s = MetadataSettings::default();
        s.genres.use_genres = true;
        f(&mut s);
        release(&json, &s, &[]).expect("maps").tracks[1]
            .metadata
            .get_all("genre")
            .to_vec()
    };
    assert_eq!(
        genre(&|s| {
            s.genres.min_genre_usage = 0;
            s.genres.max_genres = 2;
        }),
        ["Alternative Rock", "Art Rock"]
    );
    assert_eq!(
        genre(&|s| {
            s.genres.min_genre_usage = 0;
            s.genres.max_genres = 2;
            s.genres.join_genres = " / ".into();
        }),
        ["Alternative Rock / Art Rock"]
    );
    assert_eq!(
        genre(&|s| {
            s.genres.min_genre_usage = 0;
            s.genres.max_genres = 0;
        }),
        Vec::<String>::new()
    );
    assert_eq!(
        genre(&|s| {
            s.genres.min_genre_usage = 0;
            s.genres.max_genres = 1;
            s.genres.genres_filter = "-alternative rock".into();
        }),
        ["Art Rock"]
    );
    assert_eq!(
        genre(&|s| s.genres.use_genres = false),
        Vec::<String>::new()
    );
    let folk = genre(&|s| {
        s.genres.folksonomy_tags = true;
        s.genres.min_genre_usage = 0;
        s.genres.max_genres = 3;
    });
    assert_eq!(folk.len(), 3);
}

#[test]
fn translated_artist_names_titles_and_sort_names() {
    let json = load(UTADA);
    let map_with = |f: &dyn Fn(&mut MetadataSettings)| {
        let mut s = MetadataSettings::default();
        f(&mut s);
        release(&json, &s, &[]).expect("maps")
    };
    // Default (variations): the credit is not an ended alias, so the artist's own name and sort name.
    let plain = map_with(&|_| {});
    assert_eq!(plain.metadata.get("albumartist"), Some("宇多田ヒカル"));
    assert_eq!(plain.metadata.get("albumartistsort"), Some("Utada, Hikaru"));
    assert_eq!(plain.metadata.get_all("~albumartists_countries"), ["JP"]);
    // Credited names: the sort name of the alias spelled like the credit, like Picard.
    let credited = map_with(&|s| s.standardize_artist_names = StandardizeArtistNames::None);
    assert_eq!(
        credited.metadata.get("albumartistsort"),
        Some("うただヒカル")
    );
    // English translation uses the primary `en` artist-name alias.
    let en = map_with(&|s| s.translate_artist_names = true);
    assert_eq!(en.metadata.get("albumartist"), Some("Hikaru Utada"));
    assert_eq!(en.metadata.get("albumartistsort"), Some("Utada, Hikaru"));
    assert_eq!(en.tracks[0].metadata.get("artist"), Some("Hikaru Utada"));
    assert_eq!(en.tracks[0].metadata.get_all("artists"), ["Hikaru Utada"]);
    // en_PH is preferred when asked for.
    let ph = map_with(&|s| {
        s.translate_artist_names = true;
        s.translation_locales = vec!["en_PH".into()];
    });
    assert_eq!(ph.metadata.get("albumartist"), Some("Utada Hikaru"));
    // Script exception: a Japanese-script name is not translated.
    let exception = map_with(&|s| {
        s.translate_artist_names = true;
        s.translate_artist_names_script_exception = true;
        s.script_exceptions = vec![ScriptException {
            script: "CJK".into(),
            weighting: 10,
        }];
    });
    assert_eq!(exception.metadata.get("albumartist"), Some("宇多田ヒカル"));
    assert!(
        !plain.tracks[0].metadata.contains("genre"),
        "genres only when asked for"
    );

    // Titles of releases and recordings come from locale aliases.
    let mut crafted = json.clone();
    crafted["aliases"] = json!([
        {"name": "Addicted to You (EN)", "sort-name": "x", "locale": "en", "primary": true, "type-id": null, "ended": false}
    ]);
    crafted["media"][0]["tracks"][0]["recording"]["aliases"] = json!([
        {"name": "Recording EN", "sort-name": "x", "locale": "en", "primary": true,
         "type-id": "5d564c8f-97de-3572-94bb-7f40ad661499", "ended": false}
    ]);
    let mut s = MetadataSettings {
        translate_album_titles: true,
        translate_track_titles: true,
        ..MetadataSettings::default()
    };
    let titled = release(&crafted, &s, &[]).expect("maps");
    assert_eq!(titled.metadata.get("album"), Some("Addicted to You (EN)"));
    assert_eq!(titled.tracks[0].metadata.get("title"), Some("Recording EN"));
    assert_eq!(
        titled.tracks[0].metadata.get("~recordingtitle"),
        Some("Recording EN")
    );
    assert_eq!(
        titled.tracks[1].metadata.get("title"),
        Some("Addicted to You (Underwater mix)"),
        "no alias, no change"
    );
    s.translate_album_titles = false;
    s.translate_track_titles = false;
    assert_eq!(
        release(&crafted, &s, &[])
            .expect("maps")
            .metadata
            .get("album"),
        Some("Addicted to You")
    );
}

#[test]
fn various_artists_release_is_a_compilation_with_a_custom_name_and_disc_ids() {
    let json = load(NOW_37);
    let settings = MetadataSettings {
        va_name: "VA".into(),
        ..MetadataSettings::default()
    };
    let second_disc = json["media"][1]["discs"][0]["id"]
        .as_str()
        .expect("disc id")
        .to_owned();
    let mapped = release(&json, &settings, std::slice::from_ref(&second_disc)).expect("maps");
    let album = &mapped.metadata;
    assert_eq!(album.get("albumartist"), Some("VA"));
    assert_eq!(album.get("albumartistsort"), Some("VA"));
    assert_eq!(album.get("compilation"), Some("1"));
    assert_eq!(all(album, "releasetype"), ["album", "compilation"]);
    assert_eq!(album.get("~secondaryreleasetype"), Some("compilation"));
    assert_eq!(album.get("~totalalbumtracks"), Some("6"));
    assert_eq!(album.get_all("~musicbrainz_discids").len(), 2);
    assert_eq!(mapped.tracks.len(), 6);
    for (i, t) in mapped.tracks.iter().enumerate() {
        let m = &t.metadata;
        assert_eq!(m.get("compilation"), Some("1"), "track {i}");
        assert_eq!(
            m.get("~absolutetracknumber"),
            Some((i + 1).to_string().as_str())
        );
        assert_eq!(m.get("~multiartist"), Some("1"));
        assert_eq!(m.get("albumartist"), Some("VA"));
        assert_ne!(
            m.get("artist"),
            Some("VA"),
            "track artists are real artists"
        );
    }
    let first_disc: Vec<_> = mapped.tracks[..3]
        .iter()
        .map(|t| t.metadata.get("musicbrainz_discid"))
        .collect();
    assert_eq!(first_disc, [None, None, None]);
    for t in &mapped.tracks[3..] {
        assert_eq!(
            t.metadata.get_all("musicbrainz_discid"),
            std::slice::from_ref(&second_disc)
        );
    }
    assert_eq!(mapped.tracks[3].metadata.get("tracknumber"), Some("1"));
    assert_eq!(mapped.tracks[3].metadata.get("discnumber"), Some("2"));

    // A track credited to Various Artists gets the custom name too.
    let mut va_track = json.clone();
    va_track["media"][0]["tracks"][0]["artist-credit"][0]["artist"]["id"] =
        json!(VARIOUS_ARTISTS_ID);
    va_track["media"][0]["tracks"][0]["artist-credit"][0]["name"] = json!("Various Artists");
    let mapped = release(&va_track, &settings, &[]).expect("maps");
    assert_eq!(mapped.tracks[0].metadata.get("artist"), Some("VA"));
    assert_eq!(mapped.tracks[0].metadata.get("artistsort"), Some("VA"));
}

#[test]
fn special_tracks_flags_and_punctuation() {
    let track = |id: &str, pos: u32, title: &str| {
        json!({"id": id, "position": pos, "number": pos.to_string(), "title": title,
               "recording": {"id": format!("r-{id}"), "title": title, "video": id == "v"}})
    };
    let json = json!({
        "id": "rel", "title": "Don’t “Stop” – Now", "release-group": {"id": "rg"},
        "artist-credit": [{"name": "A", "artist": {"id": "a", "name": "A", "sort-name": "A"}}],
        "media": [{
            "position": 1, "format": "CD", "track-count": 3,
            "pregap": track("p", 0, "Hidden “pregap”"),
            "tracks": [track("v", 1, "Video — clip"), track("s", 2, "[silence]")],
            "data-tracks": [track("d", 3, "[data track]")]
        }]
    });
    let off = release(&json, &MetadataSettings::default(), &[]).expect("maps");
    let titles: Vec<_> = off
        .tracks
        .iter()
        .map(|t| t.metadata.get("title").unwrap_or(""))
        .collect();
    assert_eq!(
        titles,
        [
            "Hidden “pregap”",
            "Video — clip",
            "[silence]",
            "[data track]"
        ]
    );
    assert_eq!(off.tracks[0].metadata.get("~pregap"), Some("1"));
    assert_eq!(off.tracks[0].metadata.get("~discpregap"), Some("1"));
    assert_eq!(
        off.tracks[1].metadata.get("~discpregap"),
        Some("1"),
        "the medium has a pregap"
    );
    assert_eq!(off.tracks[1].metadata.get("~video"), Some("1"));
    assert_eq!(off.tracks[2].metadata.get("~silence"), Some("1"));
    assert_eq!(off.tracks[3].metadata.get("~datatrack"), Some("1"));
    assert_eq!(off.tracks[1].metadata.get("~pregap"), None);
    assert_eq!(off.tracks[1].metadata.get("~datatrack"), None);
    let numbers: Vec<_> = off
        .tracks
        .iter()
        .map(|t| t.metadata.get("~absolutetracknumber").unwrap_or(""))
        .collect();
    assert_eq!(numbers, ["1", "2", "3", "4"]);
    assert_eq!(off.metadata.get("~totalalbumtracks"), Some("4"));
    assert_eq!(
        off.tracks[0].metadata.get("totaltracks"),
        Some("4"),
        "3 tracks + pregap"
    );
    assert_eq!(off.metadata.get("album"), Some("Don’t “Stop” – Now"));

    let on = release(
        &json,
        &MetadataSettings {
            convert_punctuation: true,
            ..MetadataSettings::default()
        },
        &[],
    )
    .expect("maps");
    assert_eq!(on.metadata.get("album"), Some("Don't \"Stop\" - Now"));
    assert_eq!(
        on.tracks[0].metadata.get("title"),
        Some("Hidden \"pregap\"")
    );
    assert_eq!(
        on.tracks[1].metadata.get("album"),
        Some("Don't \"Stop\" - Now")
    );
    assert_eq!(on.tracks[1].metadata.get("title"), Some("Video - clip"));
}

#[test]
fn rating_comment_and_release_extras() {
    let mut json = load(OK_COMPUTER);
    json["annotation"] = json!("An annotation");
    json["disambiguation"] = json!("2xCD edition");
    json["release-group"]["disambiguation"] = json!("rg comment");
    json["media"][0]["tracks"][0]["recording"]["user-rating"] =
        json!({"value": 4, "votes-count": 1});
    json["media"][0]["tracks"][0]["recording"]["disambiguation"] = json!("album version");
    let mapped = release(&json, &MetadataSettings::default(), &[]).expect("maps");
    assert_eq!(
        mapped.metadata.get("~releaseannotation"),
        Some("An annotation")
    );
    assert_eq!(mapped.metadata.get("~releasecomment"), Some("2xCD edition"));
    assert_eq!(
        mapped.metadata.get("~releasegroupcomment"),
        Some("rg comment")
    );
    let t = &mapped.tracks[0].metadata;
    assert_eq!(t.get("~rating"), Some("4"));
    assert_eq!(t.get("~recordingcomment"), Some("album version"));
    assert!(!mapped.tracks[1].metadata.contains("~rating"));
}

#[test]
fn release_ars_off_skips_release_and_group_relationships() {
    let json = load(OK_COMPUTER);
    let off = release(
        &json,
        &MetadataSettings {
            release_ars: false,
            ..MetadataSettings::default()
        },
        &[],
    )
    .expect("maps");
    assert!(!off.metadata.contains("producer"));
    assert!(
        off.tracks[1].metadata.contains("writer"),
        "recording and work relationships are always mapped"
    );
}

#[test]
fn composer_and_lyricist_from_work_relationships() {
    let rec = json!({
        "id": "rec", "title": "Song",
        "relations": [{
            "target-type": "work", "type": "performance", "attributes": [],
            "work": {
                "id": "work-1", "title": "Song (work)", "languages": ["eng", "fra"],
                "relations": [
                    {"target-type": "artist", "type": "composer", "attributes": [], "target-credit": "J. Smith",
                     "artist": {"id": "a1", "name": "John Smith", "sort-name": "Smith, John"}},
                    {"target-type": "artist", "type": "lyricist", "attributes": [],
                     "artist": {"id": "a2", "name": "Jane Doe", "sort-name": "Doe, Jane"}},
                    {"target-type": "artist", "type": "librettist", "attributes": [],
                     "artist": {"id": "a2", "name": "Jane Doe", "sort-name": "Doe, Jane"}}
                ]
            }
        }]
    });
    let credited = with(StandardizeArtistNames::None);
    let mut m = Metadata::new();
    recording(&rec, &mut m, &credited);
    assert_eq!(
        all(&m, "composer"),
        ["J. Smith"],
        "credited name used unless standardising"
    );
    assert_eq!(all(&m, "composersort"), ["Smith, John"]);
    assert_eq!(all(&m, "musicbrainz_composerid"), ["a1"]);
    assert_eq!(all(&m, "lyricist"), ["Jane Doe"], "duplicates collapse");
    assert_eq!(all(&m, "~lyricistsort"), ["Doe, Jane"]);
    assert_eq!(all(&m, "language"), ["eng", "fra"]);
    assert_eq!(all(&m, "work"), ["Song (work)"]);

    // Instrumental performances: no lyricist, language "zxx".
    let mut instrumental = rec.clone();
    instrumental["relations"][0]["attributes"] = json!(["instrumental"]);
    let mut m = Metadata::new();
    recording(&instrumental, &mut m, &with(StandardizeArtistNames::All));
    assert!(!m.contains("lyricist"));
    assert_eq!(all(&m, "language"), ["zxx"]);
    assert_eq!(all(&m, "~performance_attributes"), ["instrumental"]);
    assert_eq!(
        all(&m, "composer"),
        ["John Smith"],
        "standardised artist name"
    );
}

#[test]
fn release_level_mix_dj_applies_to_its_medium_only() {
    let json = json!({
        "id": "rel", "title": "Mix", "release-group": {"id": "rg"},
        "media": [
            {"position": 1, "format": "CD", "track-count": 1, "tracks": [
                {"id": "t1", "position": 1, "title": "One", "recording": {"id": "r1", "title": "One"}}]},
            {"position": 2, "format": "CD", "track-count": 1, "tracks": [
                {"id": "t2", "position": 1, "title": "Two", "recording": {"id": "r2", "title": "Two"}}]}
        ],
        "relations": [
            {"target-type": "artist", "type": "producer", "attributes": [],
             "artist": {"id": "p", "name": "Pat", "sort-name": "Pat"}},
            {"target-type": "artist", "type": "mix-DJ", "attributes": ["medium 2"],
             "artist": {"id": "d", "name": "DJ Two", "sort-name": "DJ Two"}}
        ]
    });
    let mapped = release(&json, &MetadataSettings::default(), &[]).expect("maps");
    assert_eq!(all(&mapped.metadata, "producer"), ["Pat"]);
    assert!(!mapped.metadata.contains("djmixer"));
    assert!(!mapped.tracks[0].metadata.contains("djmixer"));
    assert_eq!(all(&mapped.tracks[1].metadata, "djmixer"), ["DJ Two"]);
}

#[test]
fn preferred_country_and_pregap_and_missing_recording() {
    let mut json = load(OK_COMPUTER);
    json["release-events"] = json!([
        {"date": "1997-06-16", "area": {"iso-3166-1-codes": ["GB"]}},
        {"date": "1997-07-01", "area": {"iso-3166-1-codes": ["US"]}}
    ]);
    let settings = MetadataSettings {
        preferred_countries: vec!["FR".into(), "GB".into()],
        ..MetadataSettings::default()
    };
    let mapped = release(&json, &settings, &[]).expect("maps");
    assert_eq!(mapped.metadata.get("releasecountry"), Some("GB"));
    assert_eq!(all(&mapped.metadata, "~releasecountries"), ["GB", "US"]);
    let settings = MetadataSettings {
        preferred_countries: vec!["FR".into()],
        ..MetadataSettings::default()
    };
    assert_eq!(
        release(&json, &settings, &[])
            .expect("maps")
            .metadata
            .get("releasecountry"),
        Some("US")
    );

    // A hidden pregap track counts towards the medium's total and is listed first.
    let mut with_pregap = load(OK_COMPUTER);
    with_pregap["media"][0]["pregap"] = json!({
        "id": "pregap", "position": 0, "number": "0", "title": "Hidden",
        "recording": {"id": "pregap-rec", "title": "Hidden", "length": 5000}
    });
    let r = release(&with_pregap, &MetadataSettings::default(), &[]).expect("maps");
    assert_eq!(r.tracks.len(), 13);
    assert_eq!(r.tracks[0].metadata.get("tracknumber"), Some("0"));
    assert_eq!(r.tracks[0].metadata.get("totaltracks"), Some("13"));

    let mut broken = load(OK_COMPUTER);
    broken["media"][0]["tracks"][0]
        .as_object_mut()
        .expect("object")
        .remove("recording");
    assert!(matches!(
        release(&broken, &MetadataSettings::default(), &[]),
        Err(Error::Response(_))
    ));
    assert!(matches!(
        release(&json!({"title": "x"}), &MetadataSettings::default(), &[]),
        Err(Error::Response(_))
    ));
}

#[test]
fn standalone_recording_uses_the_nat_name_and_first_release_date() {
    let json = load(RECORDING_LOOKUP);
    let settings = MetadataSettings::default();
    let track = standalone_recording(&json, &settings).expect("maps");
    let m = &track.metadata;
    assert_eq!(track.id, "9f9cf187-d6f9-437f-9d98-d59cdbd52757");
    assert_eq!(m.get("album"), Some("[standalone recordings]"));
    assert_eq!(m.get("title"), Some("Paranoid Android"));
    assert_eq!(m.get("artist"), Some("Radiohead"));
    assert_eq!(m.get("originaldate"), Some("1997-05-21"));
    assert_eq!(m.get("originalyear"), Some("1997"));
    assert_eq!(
        m.get("musicbrainz_recordingid"),
        Some("9f9cf187-d6f9-437f-9d98-d59cdbd52757")
    );
    assert!(!m.contains("tracknumber") && !m.contains("musicbrainz_albumid"));
    let renamed = MetadataSettings {
        nat_name: "Singles".into(),
        ..MetadataSettings::default()
    };
    assert_eq!(
        standalone_recording(&json, &renamed)
            .expect("maps")
            .metadata
            .get("album"),
        Some("Singles")
    );
    assert!(matches!(
        standalone_recording(&json!({}), &settings),
        Err(Error::Response(_))
    ));
}

#[test]
fn release_search_hits_become_candidates() {
    let hits = load(RELEASE_SEARCH);
    let candidates: Vec<_> = array(&hits, "releases")
        .iter()
        .filter_map(release_candidate)
        .collect();
    assert_eq!(candidates.len(), 2);
    let top = &candidates[0];
    assert_eq!(
        top.id,
        ReleaseId {
            source: Source::MusicBrainz,
            id: "6a09041b-0f79-3278-88d9-0552dd28d37f".into()
        }
    );
    assert_eq!(top.search_score, 100);
    let m = &top.metadata;
    assert_eq!(m.get("album"), Some("OK Computer"));
    assert_eq!(m.get("albumartist"), Some("Radiohead"));
    assert_eq!(m.get("date"), Some("1997-07-01"));
    assert_eq!(m.get("releasecountry"), Some("US"));
    assert_eq!(m.get("totaltracks"), Some("12"));
    assert_eq!(m.get("totaldiscs"), Some("1"));
    assert_eq!(m.get("media"), Some("CD"));
    assert_eq!(m.get_all("~mediaformats"), ["CD"]);
    assert_eq!(m.get("barcode"), Some("724385522925"));
    assert_eq!(m.get("releasetype"), Some("album"));
    assert_eq!(m.get("releasestatus"), Some("official"));
    assert_eq!(
        m.get("musicbrainz_releasegroupid"),
        Some("b1392450-e666-3926-a536-22c65f834433")
    );
    assert_eq!(m.get("label"), Some("Capitol Records"));
    assert_eq!(m.get("catalognumber"), Some("CDP 7243 8 55229 2 5"));

    // A two-medium release without date/country.
    let digital = &candidates[1];
    assert_eq!(digital.id.id, "4513f772-f496-46f5-a82d-7ab1f4c59849");
    assert_eq!(digital.metadata.get("media"), Some("2×Digital Media"));
    assert_eq!(
        digital.metadata.get_all("~mediaformats"),
        ["Digital Media", "Digital Media"]
    );
    assert_eq!(digital.metadata.get("totaldiscs"), Some("2"));
    assert_eq!(
        digital.metadata.get("totaltracks"),
        Some("19"),
        "total over both media"
    );
    assert!(!digital.metadata.contains("date"));
    assert!(!digital.metadata.contains("releasecountry"));
}

#[test]
fn recording_search_hits_are_flattened_per_release() {
    let hits = load(RECORDING_SEARCH);
    let candidates: Vec<_> = array(&hits, "recordings")
        .iter()
        .flat_map(recording_candidates)
        .collect();
    assert_eq!(
        candidates.len(),
        4,
        "2 + 1 releases, plus 1 standalone recording"
    );

    let mix = &candidates[0];
    assert_eq!(mix.recording_id, "d1400ebf-fcdb-43fa-947b-073e6ec190df");
    assert_eq!(
        mix.release_id.as_deref(),
        Some("7f904fa5-1859-42bf-83be-256c3bf99c2c")
    );
    assert_eq!(mix.search_score, 100);
    let m = &mix.metadata;
    assert_eq!(m.get("title"), Some("Paranoid Android"));
    assert_eq!(m.get("artist"), Some("Radiohead"));
    assert_eq!(m.length(), Some(Duration::from_millis(384_000)));
    assert_eq!(m.get("album"), Some("The Mix 193"));
    assert_eq!(
        m.get("tracknumber"),
        Some("16"),
        "track-offset 15 is position 16"
    );
    assert_eq!(m.get("totaltracks"), Some("20"));
    assert_eq!(m.get("discnumber"), Some("1"));
    assert_eq!(m.get("media"), Some("CD"));
    assert_eq!(m.get("date"), Some("1997-12"));
    assert_eq!(m.get("releasestatus"), Some("promotion"));
    assert_eq!(
        m.get("musicbrainz_albumid"),
        Some("7f904fa5-1859-42bf-83be-256c3bf99c2c")
    );
    assert_eq!(
        m.get("musicbrainz_recordingid"),
        Some("d1400ebf-fcdb-43fa-947b-073e6ec190df")
    );
    assert!(!m.contains("isrc"), "null isrcs");

    // Same recording on another release: its own position, no medium format.
    let bootleg = &candidates[1];
    assert_eq!(bootleg.recording_id, mix.recording_id);
    assert_eq!(
        bootleg.release_id.as_deref(),
        Some("38bac4ea-3b88-4f12-abdf-f8ae08bfd684")
    );
    assert_eq!(bootleg.metadata.get("tracknumber"), Some("8"));
    assert!(!bootleg.metadata.contains("media"));
    assert!(!bootleg.metadata.contains("date"));

    // Second disc of a two-disc release; search isrcs are `{"id": ...}` objects.
    let beacon = &candidates[2];
    assert_eq!(beacon.recording_id, "71633c98-eaf8-4a11-a393-0f70eff53cfa");
    assert_eq!(
        beacon.release_id.as_deref(),
        Some("0c1ef0ab-7183-4620-814f-0c9f978200ff")
    );
    let m = &beacon.metadata;
    assert_eq!(
        (
            m.get("discnumber"),
            m.get("tracknumber"),
            m.get("totaltracks")
        ),
        (Some("2"), Some("2"), Some("12"))
    );
    assert_eq!(all(m, "isrc"), ["GBAYE0300123"]);
    assert_eq!(m.get("releasecountry"), Some("XE"));
    assert_eq!(m.length(), Some(Duration::from_millis(386_373)));

    let standalone = &candidates[3];
    assert_eq!(standalone.release_id, None);
    assert_eq!(standalone.search_score, 87);
    assert!(!standalone.metadata.contains("album"));
    assert_eq!(
        standalone.metadata.get("title"),
        Some("Paranoid Android (demo)")
    );
    assert_eq!(
        standalone.metadata.get("musicbrainz_recordingid"),
        Some("00000000-0000-4000-8000-000000000002")
    );
}
