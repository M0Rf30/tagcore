//! "Other versions" of a release: the releases of a release group, with the display names of
//! Picard's `releasegroup.py` (`prepare_releases_for_versions`, `_parse_versions`).

use serde_json::Value;

use super::map::{media_formats, release_countries};
use super::{array, str_field};

/// Picard's `VERSIONS_MAX_TRACKS`: media beyond this many are summarised with `+…`.
const VERSIONS_MAX_TRACKS: usize = 10;

/// One release of a release group.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseVersion {
    /// Release MBID.
    pub id: String,
    /// `tracks / year / country / format / label / catnum`, with the differing barcode, packaging
    /// or disambiguation appended when several versions would otherwise look the same.
    pub name: String,
    /// Packaging, barcode and disambiguation, one `Key: value` per line (only the present ones).
    pub extra: String,
    pub totaltracks: u32,
    pub countries: Vec<String>,
    pub formats: Vec<String>,
    /// Release date as the web service gave it (`""` when unknown).
    pub date: String,
    /// Track count per medium, `12+8`.
    pub tracks: String,
    /// `GB+US` style summary of the release events' countries (`??` when unknown).
    pub country: String,
    /// `2×CD + DVD-Video` style summary.
    pub format: String,
    pub label: String,
    pub catalog_number: String,
    pub barcode: String,
    pub packaging: String,
    pub disambiguation: String,
}

/// Picard's `limited_join` with `limit = 6`, `'+'` and `'…'`.
fn countries_shortlist(countries: &[String]) -> String {
    const LIMIT: usize = 6;
    if countries.len() <= LIMIT {
        return countries.join("+");
    }
    let half = LIMIT / 2;
    let mut parts: Vec<&str> = countries[..half].iter().map(String::as_str).collect();
    parts.push("…");
    parts.extend(
        countries[countries.len() - half..]
            .iter()
            .map(String::as_str),
    );
    parts.join("+")
}

fn unique<'a>(values: impl Iterator<Item = &'a str>) -> Vec<&'a str> {
    let mut out: Vec<&str> = Vec::new();
    for value in values {
        if !out.contains(&value) {
            out.push(value);
        }
    }
    out
}

/// Internal row: a version plus its pending disambiguation values.
struct Row {
    version: ReleaseVersion,
    short_name: String,
    disambiguate: Vec<String>,
}

fn prepare(node: &Value) -> Option<Row> {
    let id = str_field(node, "id")?;
    let label_infos = array(node, "label-info");
    let labels = unique(
        label_infos
            .iter()
            .filter_map(|l| str_field(&l["label"], "name")),
    );
    let catnums = unique(
        label_infos
            .iter()
            .filter_map(|l| str_field(l, "catalog-number")),
    );

    let countries = release_countries(node);
    let country = if countries.is_empty() {
        str_field(node, "country").unwrap_or("??").to_owned()
    } else {
        countries_shortlist(&countries)
    };

    let media = array(node, "media");
    let counts: Vec<String> = media
        .iter()
        .map(|m| m["track-count"].as_u64().unwrap_or(0).to_string())
        .collect();
    let tracks = if counts.len() > VERSIONS_MAX_TRACKS {
        format!("{}+…", counts[..VERSIONS_MAX_TRACKS].join("+"))
    } else {
        counts.join("+")
    };
    let formats: Vec<String> = media
        .iter()
        .filter_map(|m| str_field(m, "format"))
        .map(str::to_owned)
        .collect();
    let date = node.get("date").and_then(Value::as_str);
    let year = date.map_or_else(|| "????".to_owned(), |d| d.chars().take(4).collect());
    let format = media_formats(media);
    // First two words of each label name, like Picard.
    let label = labels
        .iter()
        .map(|l| l.split(' ').take(2).collect::<Vec<_>>().join(" "))
        .collect::<Vec<_>>()
        .join(", ");
    let catalog_number = catnums.join(", ");
    let short_name = [
        tracks.as_str(),
        &year,
        &country,
        &format,
        &label,
        &catalog_number,
    ]
    .join(" / ");

    Some(Row {
        short_name,
        disambiguate: Vec::new(),
        version: ReleaseVersion {
            id: id.to_owned(),
            name: String::new(),
            extra: String::new(),
            totaltracks: u32::try_from(
                media
                    .iter()
                    .map(|m| m["track-count"].as_u64().unwrap_or(0))
                    .sum::<u64>(),
            )
            .unwrap_or(u32::MAX),
            countries,
            formats,
            date: date.unwrap_or_default().to_owned(),
            tracks,
            country,
            format,
            label,
            catalog_number,
            barcode: str_field(node, "barcode").unwrap_or_default().to_owned(),
            packaging: str_field(node, "packaging").unwrap_or_default().to_owned(),
            disambiguation: str_field(node, "disambiguation")
                .unwrap_or_default()
                .to_owned(),
        },
    })
}

/// Picard's `ReleaseGroup._parse_versions` over one or more pages of `/release?release-group=..`
/// responses (`inc=media+labels`): releases are grouped by display name (in order of first
/// appearance); same-named versions are told apart by the first differing one of barcode,
/// packaging and disambiguation.
pub fn parse_versions(pages: &[Value]) -> Vec<ReleaseVersion> {
    let mut groups: Vec<(String, Vec<Row>)> = Vec::new();
    for node in pages.iter().flat_map(|page| array(page, "releases")) {
        let Some(row) = prepare(node) else { continue };
        let mut name = row.short_name.clone();
        if name == row.version.tracks {
            name = format!("[no release info] / {name}");
        }
        match groups.iter_mut().find(|(n, _)| *n == name) {
            Some((_, rows)) => rows.push(row),
            None => groups.push((name, vec![row])),
        }
    }

    let mut out = Vec::new();
    for (name, mut rows) in groups {
        // De-duplicate names where possible, comparing every pair like Picard's `combinations`.
        for i in 0..rows.len() {
            for j in i + 1..rows.len() {
                let (a, b) = (&rows[i].version, &rows[j].version);
                let keys = [
                    (barcode_text(a), barcode_text(b)),
                    (packaging_text(a), packaging_text(b)),
                    (a.disambiguation.clone(), b.disambiguation.clone()),
                ];
                let mut for_i = Vec::new();
                let mut for_j = Vec::new();
                for (x, y) in keys {
                    if x != y {
                        for_i.push(x);
                        for_j.push(y);
                    }
                }
                rows[i].disambiguate.extend(for_i);
                rows[j].disambiguate.extend(for_j);
            }
        }
        for mut row in rows {
            let dis = unique(
                row.disambiguate
                    .iter()
                    .map(String::as_str)
                    .filter(|s| !s.is_empty()),
            )
            .join(" / ");
            row.version.name = if dis.is_empty() {
                name.clone()
            } else {
                format!("{name} / {dis}")
            };
            row.version.extra = [
                ("Packaging", packaging_text(&row.version)),
                ("Barcode", barcode_text(&row.version)),
                ("Disambiguation", row.version.disambiguation.clone()),
            ]
            .iter()
            .filter(|(_, v)| !v.is_empty())
            .map(|(k, v)| format!("{k}: {v}"))
            .collect::<Vec<_>>()
            .join("\n");
            out.push(row.version);
        }
    }
    out
}

/// Barcode as Picard compares and shows it: `[no barcode]` when missing.
fn barcode_text(v: &ReleaseVersion) -> String {
    if v.barcode.is_empty() {
        "[no barcode]".to_owned()
    } else {
        v.barcode.clone()
    }
}

/// Packaging as Picard compares and shows it: `??` when missing.
fn packaging_text(v: &ReleaseVersion) -> String {
    if v.packaging.is_empty() {
        "??".to_owned()
    } else {
        v.packaging.clone()
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn release(
        id: &str,
        date: Option<&str>,
        country: &str,
        format: &str,
        tracks: u32,
        label: &str,
        catno: &str,
    ) -> Value {
        let mut node = json!({
            "id": id, "country": country,
            "release-events": [{"area": {"iso-3166-1-codes": [country]}}],
            "label-info": [{"catalog-number": catno, "label": {"name": label}}],
            "media": [{"format": format, "track-count": tracks}],
        });
        if let Some(date) = date {
            node["date"] = json!(date);
        }
        node
    }

    #[test]
    fn names_follow_picard_and_same_names_are_disambiguated() {
        let mut a = release(
            "a",
            Some("1997-07-01"),
            "US",
            "CD",
            12,
            "Capitol Records Inc",
            "CDP 1",
        );
        let mut b = a.clone();
        b["id"] = json!("b");
        a["barcode"] = json!("111");
        b["barcode"] = json!("222");
        let c = release("c", None, "XW", "Digital Media", 12, "", "");
        let pages = [json!({"release-count": 3, "releases": [a, b, c]})];
        let versions = parse_versions(&pages);
        assert_eq!(versions.len(), 3);
        assert_eq!(
            versions[0].name,
            "12 / 1997 / US / CD / Capitol Records / CDP 1 / 111"
        );
        assert_eq!(
            versions[1].name,
            "12 / 1997 / US / CD / Capitol Records / CDP 1 / 222"
        );
        assert_eq!(versions[0].extra, "Packaging: ??\nBarcode: 111");
        assert_eq!(versions[2].name, "12 / ???? / XW / Digital Media /  / ");
        assert_eq!(versions[2].totaltracks, 12);
        assert_eq!(versions[2].formats, ["Digital Media"]);
        assert_eq!(versions[0].tracks, "12");
        assert_eq!(versions[0].date, "1997-07-01");
    }

    #[test]
    fn no_release_info_and_long_lists_are_summarised() {
        let bare = json!({"id": "x", "media": [{"track-count": 5}]});
        let v = &parse_versions(&[json!({"releases": [bare]})])[0];
        assert_eq!(v.country, "??");
        assert!(v.name.contains(" / ????"), "{}", v.name);
        let many: Vec<Value> = (0..12)
            .map(|_| json!({"format": "CD", "track-count": 1}))
            .collect();
        let node = json!({"id": "y", "media": many});
        assert_eq!(
            parse_versions(&[json!({"releases": [node]})])[0].tracks,
            "1+1+1+1+1+1+1+1+1+1+…"
        );
        assert_eq!(
            countries_shortlist(&["A", "B", "C", "D", "E", "F", "G"].map(String::from)),
            "A+B+C+…+E+F+G"
        );
        assert_eq!(
            countries_shortlist(&["A".to_owned(), "B".to_owned()]),
            "A+B"
        );
    }

    #[test]
    fn pages_are_concatenated() {
        let one = json!({"releases": [release("a", Some("2000"), "GB", "CD", 3, "L", "1")]});
        let two = json!({"releases": [release("b", Some("2001"), "GB", "CD", 3, "L", "2")]});
        let ids: Vec<_> = parse_versions(&[one, two])
            .into_iter()
            .map(|v| v.id)
            .collect();
        assert_eq!(ids, ["a", "b"]);
    }
}
