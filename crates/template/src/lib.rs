// SPDX-License-Identifier: Apache-2.0
//! Storage templates such as `{{y}}/{{MM}}.{{MMMM}}/{{y}}-{{MM}}-{{dd}}/{{filename}}`, and finding an original
//! from a capture date and a file name — the fallback when a checksum lookup isn't possible.

use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Date {
    pub year: i32,
    pub month: u32,
    pub day: u32,
}

const MONTHS_EN: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];
const MONTHS_FR: [&str; 12] = [
    "janvier",
    "février",
    "mars",
    "avril",
    "mai",
    "juin",
    "juillet",
    "août",
    "septembre",
    "octobre",
    "novembre",
    "décembre",
];
const MONTHS_DE: [&str; 12] = [
    "Januar",
    "Februar",
    "März",
    "April",
    "Mai",
    "Juni",
    "Juli",
    "August",
    "September",
    "Oktober",
    "November",
    "Dezember",
];
const MONTHS_ES: [&str; 12] = [
    "enero",
    "febrero",
    "marzo",
    "abril",
    "mayo",
    "junio",
    "julio",
    "agosto",
    "septiembre",
    "octubre",
    "noviembre",
    "diciembre",
];
const MONTHS_IT: [&str; 12] = [
    "gennaio",
    "febbraio",
    "marzo",
    "aprile",
    "maggio",
    "giugno",
    "luglio",
    "agosto",
    "settembre",
    "ottobre",
    "novembre",
    "dicembre",
];
const MONTHS_PT: [&str; 12] = [
    "janeiro",
    "fevereiro",
    "março",
    "abril",
    "maio",
    "junho",
    "julho",
    "agosto",
    "setembro",
    "outubro",
    "novembro",
    "dezembro",
];
const MONTHS_NL: [&str; 12] = [
    "januari",
    "februari",
    "maart",
    "april",
    "mei",
    "juni",
    "juli",
    "augustus",
    "september",
    "oktober",
    "november",
    "december",
];
/// Every language's month names, for matching folders on disk: a month folder is recognised in any of them.
const MONTHS_ALL: [&[&str; 12]; 7] = [
    &MONTHS_EN, &MONTHS_FR, &MONTHS_DE, &MONTHS_ES, &MONTHS_IT, &MONTHS_PT, &MONTHS_NL,
];

/// The month names a template's `month_locale` writes with: its first two letters pick the language, English
/// when none is known.
fn months_for(locale: &str) -> &'static [&'static str; 12] {
    match locale.get(..2).map(str::to_ascii_lowercase).as_deref() {
        Some("fr") => &MONTHS_FR,
        Some("de") => &MONTHS_DE,
        Some("es") => &MONTHS_ES,
        Some("it") => &MONTHS_IT,
        Some("pt") => &MONTHS_PT,
        Some("nl") => &MONTHS_NL,
        _ => &MONTHS_EN,
    }
}

fn days_in_month(y: i32, m: u32) -> u32 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 => 29,
        _ => 28,
    }
}

impl Date {
    /// The calendar date of a Unix timestamp, read as UTC. `RawInfo::captured_unix` is the EXIF wall clock
    /// taken as UTC, so this gives back the date the camera wrote — the same convention
    /// [`parse_capture_date`] follows, which is the date a storage template is keyed on.
    pub fn from_unix(secs: i64) -> Date {
        // Howard Hinnant's civil_from_days, with the era starting on 0000-03-01 so a leap day lands last.
        let days = secs.div_euclid(86_400) + 719_468;
        let era = days.div_euclid(146_097);
        let doe = days.rem_euclid(146_097);
        let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
        let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
        let year = (yoe + era * 400 + i64::from(month <= 2)) as i32;
        Date { year, month, day }
    }

    /// Days since 1970-01-01 (Hinnant's days_from_civil).
    fn days_since_epoch(self) -> i64 {
        let y = i64::from(self.year) - i64::from(self.month <= 2);
        let era = y.div_euclid(400);
        let yoe = y.rem_euclid(400);
        let m = i64::from(self.month);
        let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + i64::from(self.day) - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        era * 146_097 + doe - 719_468
    }

    /// Monday 1 … Sunday 7.
    fn weekday(self) -> i64 {
        (self.days_since_epoch() + 3).rem_euclid(7) + 1
    }

    /// The ISO 8601 week number (`{{W}}`): weeks start on Monday, and week 1 is the one with the year's first
    /// Thursday, so the first days of January can belong to the last week of the year before.
    pub fn iso_week(self) -> u32 {
        let jan1 = Date {
            year: self.year,
            month: 1,
            day: 1,
        };
        let doy = self.days_since_epoch() - jan1.days_since_epoch() + 1;
        let week = (doy - self.weekday() + 10) / 7;
        let weeks_in = |year: i32| {
            let j = Date {
                year,
                month: 1,
                day: 1,
            }
            .weekday();
            let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
            if j == 4 || (leap && j == 3) { 53 } else { 52 }
        };
        if week < 1 {
            weeks_in(self.year - 1) as u32
        } else if week > weeks_in(self.year) {
            1
        } else {
            week as u32
        }
    }

    pub fn add_days(self, delta: i32) -> Date {
        let mut d = self;
        for _ in 0..delta.unsigned_abs() {
            if delta > 0 {
                if d.day < days_in_month(d.year, d.month) {
                    d.day += 1;
                } else if d.month < 12 {
                    d = Date {
                        year: d.year,
                        month: d.month + 1,
                        day: 1,
                    };
                } else {
                    d = Date {
                        year: d.year + 1,
                        month: 1,
                        day: 1,
                    };
                }
            } else if d.day > 1 {
                d.day -= 1;
            } else if d.month > 1 {
                d = Date {
                    year: d.year,
                    month: d.month - 1,
                    day: days_in_month(d.year, d.month - 1),
                };
            } else {
                d = Date {
                    year: d.year - 1,
                    month: 12,
                    day: 31,
                };
            }
        }
        d
    }
}

/// The calendar date as written at the start of an ISO 8601 timestamp (`2018-03-27T13:11:29-04:00` → 2018-03-27),
/// deliberately not converted to UTC. Also accepts the English Long format Shortcuts produces
/// (`March 27, 2018 at 13:11:29 EDT`).
pub fn parse_capture_date(s: &str) -> Option<Date> {
    let s = s.trim();
    let b = s.as_bytes();
    if b.len() >= 10 && b[4] == b'-' && b[7] == b'-' {
        let year = s.get(0..4)?.parse().ok()?;
        let month: u32 = s.get(5..7)?.parse().ok()?;
        let day: u32 = s.get(8..10)?.parse().ok()?;
        return (1..=12).contains(&month).then_some(()).and(
            (1..=days_in_month(year, month))
                .contains(&day)
                .then_some(Date { year, month, day }),
        );
    }
    let mut parts = s.split_whitespace();
    let first = parts.next()?;
    let month = MONTHS_EN
        .iter()
        .position(|m| first.eq_ignore_ascii_case(m))? as u32
        + 1;
    let day: u32 = parts.next()?.trim_end_matches(',').parse().ok()?;
    let year: i32 = parts.next()?.parse().ok()?;
    (day >= 1 && day <= days_in_month(year, month)).then_some(Date { year, month, day })
}

/// Render the directory part of a template (everything before `{{filename}}`), for writing: where Sidecar
/// files an imported photo. Tokens with no value at import (an album, a time of day) are left as written.
pub fn render_dir(template: &str, date: Date, month_locale: &str) -> String {
    let months = months_for(month_locale);
    let month = months[date.month as usize - 1];
    let short: String = month.chars().take(3).collect();
    let dir = template.rsplit_once('/').map_or("", |(d, _)| d);
    dir.replace("{{yyyy}}", &format!("{:04}", date.year))
        .replace("{{yy}}", &format!("{:02}", date.year.rem_euclid(100)))
        .replace("{{y}}", &format!("{:04}", date.year))
        .replace("{{MMMM}}", month)
        .replace("{{MMM}}", &short)
        .replace("{{MM}}", &format!("{:02}", date.month))
        .replace("{{M}}", &date.month.to_string())
        .replace("{{WW}}", &format!("{:02}", date.iso_week()))
        .replace("{{W}}", &date.iso_week().to_string())
        .replace("{{dd}}", &format!("{:02}", date.day))
        .replace("{{d}}", &date.day.to_string())
        .replace("{{filetypefull}}", "IMAGE")
        .replace("{{filetype}}", "IMG")
}

/// A piece of one folder name in a template, as it can be matched against what is on disk.
#[derive(Clone, Debug, PartialEq)]
enum Piece {
    Text(String),
    /// A month by number, written out (`{{MMMM}}`) or abbreviated (`{{MMM}}`), in any language.
    Month(u32),
    /// A token with no value from a capture date — an album, an event, a time of day: any text.
    Any,
}

/// The folder part of `template` for `date`, one pattern per folder level. Date tokens become their text;
/// month names match in any language and any abbreviation; every other token matches anything. So a library
/// laid out as `{{yyyy}}/{{album}}/` or `{{yyyy}}/Holidays {{MMMM}}/` is searched rather than missed.
fn dir_patterns(template: &str, date: Date) -> Vec<Vec<Piece>> {
    let dir = template.rsplit_once('/').map_or("", |(d, _)| d);
    dir.split('/')
        .filter(|seg| !seg.is_empty())
        .map(|seg| {
            let mut pieces = Vec::new();
            let mut rest = seg;
            let text = |pieces: &mut Vec<Piece>, t: &str| {
                if t.is_empty() {
                    return;
                }
                match pieces.last_mut() {
                    Some(Piece::Text(prev)) => prev.push_str(t),
                    _ => pieces.push(Piece::Text(t.to_string())),
                }
            };
            while let Some(open) = rest.find("{{") {
                text(&mut pieces, &rest[..open]);
                let Some(close) = rest[open..].find("}}") else {
                    text(&mut pieces, &rest[open..]);
                    rest = "";
                    break;
                };
                let token = &rest[open + 2..open + close];
                rest = &rest[open + close + 2..];
                let value = match token {
                    "yyyy" | "y" => Some(format!("{:04}", date.year)),
                    "yy" => Some(format!("{:02}", date.year.rem_euclid(100))),
                    "MM" => Some(format!("{:02}", date.month)),
                    "M" => Some(date.month.to_string()),
                    "dd" => Some(format!("{:02}", date.day)),
                    "d" => Some(date.day.to_string()),
                    "WW" => Some(format!("{:02}", date.iso_week())),
                    "W" => Some(date.iso_week().to_string()),
                    "filetype" => Some("IMG".into()),
                    "filetypefull" => Some("IMAGE".into()),
                    _ => None,
                };
                match (value, token) {
                    (Some(v), _) => text(&mut pieces, &v),
                    (None, "MMMM" | "MMM") => pieces.push(Piece::Month(date.month)),
                    (None, _) => {
                        if pieces.last() != Some(&Piece::Any) {
                            pieces.push(Piece::Any);
                        }
                    }
                }
            }
            text(&mut pieces, rest);
            pieces
        })
        .collect()
}

/// The ways month `m` may be written in a folder name: its full name in each language, and every abbreviation
/// of three letters or more, with or without a full stop. Dutch writes March `mrt`, which is no prefix.
fn month_spellings(m: u32) -> Vec<String> {
    let mut out = Vec::new();
    for months in MONTHS_ALL {
        let full: Vec<char> = months[m as usize - 1].to_lowercase().chars().collect();
        for n in 3..=full.len() {
            let s: String = full[..n].iter().collect();
            if n < full.len() {
                out.push(format!("{s}."));
            }
            out.push(s);
        }
    }
    if m == 3 {
        out.push("mrt".into());
    }
    out.sort_by_key(|s| std::cmp::Reverse(s.len()));
    out.dedup();
    out
}

/// Does `name` (lower-cased) match `pieces` exactly?
fn matches(pieces: &[Piece], name: &str) -> bool {
    let Some((first, rest)) = pieces.split_first() else {
        return name.is_empty();
    };
    match first {
        Piece::Text(t) => name
            .strip_prefix(t.to_lowercase().as_str())
            .is_some_and(|r| matches(rest, r)),
        Piece::Month(m) => month_spellings(*m).iter().any(|s| {
            name.strip_prefix(s.as_str())
                .is_some_and(|r| matches(rest, r))
        }),
        Piece::Any => name
            .char_indices()
            .map(|(i, _)| i)
            .chain([name.len()])
            .any(|i| matches(rest, &name[i..])),
    }
}

/// The folders under `root` that `patterns` describe, level by level: an exact folder name is joined straight
/// on, as before; anything else, or one written in another case, is found by listing the level above. Hidden
/// folders are skipped and the expansion is bounded, so a wildcard over a large library stays a short walk.
fn matching_dirs(root: &Path, patterns: &[Vec<Piece>]) -> Vec<PathBuf> {
    const MAX: usize = 2_000;
    let mut level = vec![root.to_path_buf()];
    for pat in patterns {
        let mut next = Vec::new();
        for dir in &level {
            if let [Piece::Text(t)] = pat.as_slice()
                && dir.join(t).is_dir()
            {
                next.push(dir.join(t));
                continue;
            }
            let Ok(entries) = std::fs::read_dir(dir) else {
                continue;
            };
            let mut found: Vec<PathBuf> = entries
                .flatten()
                .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
                .filter_map(|e| {
                    let name = e.file_name().to_str()?.to_lowercase();
                    (!name.starts_with('.') && matches(pat, &name)).then(|| e.path())
                })
                .collect();
            found.sort();
            next.extend(found);
        }
        next.truncate(MAX);
        level = next;
    }
    level
}

pub const RAW_EXTENSIONS: &[&str] = &[
    "nef", "nrw", "arw", "srf", "sr2", "raf", "cr2", "cr3", "crw", "dng", "orf", "rw2", "pef",
    "srw", "x3f", "3fr", "iiq", "erf", "mef", "mos", "rwl",
];

pub fn is_raw(path: &Path) -> bool {
    has_extension(path, RAW_EXTENSIONS)
}

/// Already-developed formats Sidecar can also open, decoded by `sidecar_raw::decoded` rather than LibRaw.
/// HEIC belongs here even though the published image ships no HEIF decoder: whether a given deployment can
/// decode one is the worker's business, and it explains itself better than a refusal at resolution time.
pub const IMAGE_EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "tif", "tiff", "heic", "heif"];

pub fn is_image(path: &Path) -> bool {
    has_extension(path, IMAGE_EXTENSIONS)
}

/// Anything Sidecar will try to open, RAW or not.
pub fn is_editable(path: &Path) -> bool {
    is_raw(path) || is_image(path)
}

fn has_extension(path: &Path, extensions: &[&str]) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| extensions.contains(&e.to_ascii_lowercase().as_str()))
}

#[derive(Clone, Debug, PartialEq)]
pub struct Candidate {
    pub path: PathBuf,
    pub size: u64,
    /// Lower is better.
    pub rank: u32,
}

/// `DSC_1234.NEF` → `DSC_1234`; `DSC_1234+1.NEF` → `DSC_1234` (Immich's collision suffix).
pub fn stem_of(name: &str) -> &str {
    let stem = name.rsplit_once('.').map_or(name, |(s, _)| s);
    match stem.rsplit_once('+') {
        Some((base, n)) if !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()) => base,
        _ => stem,
    }
}

/// Look for `name`'s stem in the folders the template gives for `date` and the days either side. Month names
/// are recognised in any language, so the template's locale only matters when writing (`render_dir`).
/// Ranking: RAW before other formats, exact name, then matching size; the capture day before its neighbours.
pub fn find_by_date_and_name(
    root: &Path,
    template: &str,
    _month_locale: &str,
    date: Date,
    name: &str,
    size: Option<u64>,
) -> Vec<Candidate> {
    let want_stem = stem_of(name).to_ascii_lowercase();
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for (day_offset, penalty) in [(0, 0), (-1, 8), (1, 8)] {
        for dir in matching_dirs(root, &dir_patterns(template, date.add_days(day_offset))) {
            // A folder that isn't per day (an album, a month) comes up for all three days: once is enough.
            if !seen.insert(dir.clone()) {
                continue;
            }
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                let Some(fname) = path.file_name().and_then(|f| f.to_str()) else {
                    continue;
                };
                if fname.ends_with(".xmp") || stem_of(fname).to_ascii_lowercase() != want_stem {
                    continue;
                }
                let Ok(md) = entry.metadata() else { continue };
                if !md.is_file() {
                    continue;
                }
                let mut rank = penalty;
                if !is_raw(&path) {
                    rank += 4;
                }
                if !fname.eq_ignore_ascii_case(name) {
                    rank += 2;
                }
                if size.is_some_and(|s| s != md.len()) {
                    rank += 1;
                }
                out.push(Candidate {
                    path,
                    size: md.len(),
                    rank,
                });
            }
        }
    }
    out.sort_by(|a, b| a.rank.cmp(&b.rank).then_with(|| a.path.cmp(&b.path)));
    out
}

/// Look for `query` anywhere under `root`, by stem, case-insensitively and as a substring — which is what a
/// person typing into a picker means, and what Immich's own filename search does.
///
/// A library is a photo collection, so the walk is bounded rather than exhaustive: it stops once `limit`
/// matches are in hand or `MAX_DIRS` directories have been opened, and it never follows a symlink, so a link
/// pointing back up its own tree cannot spin. Directories are visited newest first, because a picker's answer
/// is far more often a recent photo than an old one, and that ordering makes the bound land in the right
/// place when it does bite. Hidden directories are skipped.
///
/// Ranking: an exact stem before a substring, RAW before anything else, then the shorter stem — a query that
/// is the whole of one name and part of another should offer the whole one first.
pub fn find_by_name(root: &Path, query: &str, limit: usize) -> Vec<Candidate> {
    const MAX_DIRS: usize = 20_000;
    let want = stem_of(query).to_ascii_lowercase();
    let mut out = Vec::new();
    if want.is_empty() {
        return out;
    }
    let (mut queue, mut opened) = (vec![root.to_path_buf()], 0usize);
    while let Some(dir) = queue.pop() {
        if out.len() >= limit || opened >= MAX_DIRS {
            break;
        }
        opened += 1;
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut subdirs = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(fname) = path.file_name().and_then(|f| f.to_str()) else {
                continue;
            };
            if fname.starts_with('.') {
                continue;
            }
            // `file_type` does not follow a symlink, so a linked directory is simply not descended into.
            let Ok(ft) = entry.file_type() else { continue };
            if ft.is_dir() {
                subdirs.push(path);
                continue;
            }
            if !ft.is_file() || !is_editable(&path) {
                continue;
            }
            let stem = stem_of(fname).to_ascii_lowercase();
            if !stem.contains(&want) {
                continue;
            }
            let Ok(md) = entry.metadata() else { continue };
            let mut rank = if stem == want { 0 } else { 8 };
            if !is_raw(&path) {
                rank += 4;
            }
            rank += (stem.len() - want.len()).min(3) as u32;
            out.push(Candidate {
                path,
                size: md.len(),
                rank,
            });
        }
        subdirs.sort();
        queue.extend(subdirs);
    }
    out.sort_by(|a, b| a.rank.cmp(&b.rank).then_with(|| a.path.cmp(&b.path)));
    out.truncate(limit);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The web app's copy of the extension lists (`web/src/lib/formats.ts`, for file pickers) and the share
    /// target's in the manifest are hand-kept; they had drifted to 10 and 11 of these 21 before this test.
    #[test]
    fn the_web_app_offers_every_extension() {
        let web = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../web");
        let formats = std::fs::read_to_string(web.join("src/lib/formats.ts")).unwrap();
        let listed = |name: &str| -> Vec<String> {
            let start = formats.find(&format!("export const {name} = [")).unwrap();
            let list = &formats[start..];
            let list = &list[list.find('[').unwrap() + 1..list.find(']').unwrap()];
            list.split(',')
                .map(|s| s.trim().trim_matches('\'').to_string())
                .filter(|s| !s.is_empty())
                .collect()
        };
        assert_eq!(listed("RAW_EXTENSIONS"), RAW_EXTENSIONS, "formats.ts");
        assert_eq!(listed("IMAGE_EXTENSIONS"), IMAGE_EXTENSIONS, "formats.ts");
        let manifest = std::fs::read_to_string(web.join("static/manifest.webmanifest")).unwrap();
        for ext in RAW_EXTENSIONS {
            assert!(
                manifest.contains(&format!("\".{ext}\"")),
                "the share target doesn't accept .{ext}"
            );
        }
    }

    const T: &str = "{{y}}/{{MM}}.{{MMMM}}/{{y}}-{{MM}}-{{dd}}/{{filename}}";

    #[test]
    fn renders_the_users_template() {
        let d = Date {
            year: 2018,
            month: 3,
            day: 27,
        };
        assert_eq!(render_dir(T, d, "en"), "2018/03.March/2018-03-27");
    }

    #[test]
    fn parses_iso_and_long_dates_without_utc_conversion() {
        assert_eq!(
            parse_capture_date("2018-03-27T22:30:00-04:00"),
            Some(Date {
                year: 2018,
                month: 3,
                day: 27
            })
        );
        assert_eq!(
            parse_capture_date("March 27, 2018 at 13:11:29 EDT"),
            Some(Date {
                year: 2018,
                month: 3,
                day: 27
            })
        );
        assert_eq!(parse_capture_date("2018-02-30T00:00:00Z"), None);
    }

    #[test]
    fn day_arithmetic_crosses_months_and_years() {
        assert_eq!(
            Date {
                year: 2024,
                month: 3,
                day: 1
            }
            .add_days(-1),
            Date {
                year: 2024,
                month: 2,
                day: 29
            }
        );
        assert_eq!(
            Date {
                year: 2025,
                month: 12,
                day: 31
            }
            .add_days(1),
            Date {
                year: 2026,
                month: 1,
                day: 1
            }
        );
    }

    #[test]
    fn finds_raw_over_camera_jpeg_and_checks_neighbouring_days() {
        let tmp = tempfile::tempdir().unwrap();
        let day = tmp.path().join("2026/08.August/2026-08-26");
        let next = tmp.path().join("2026/08.August/2026-08-27");
        std::fs::create_dir_all(&day).unwrap();
        std::fs::create_dir_all(&next).unwrap();
        std::fs::write(day.join("_D5N5068.JPG"), b"jpeg").unwrap();
        std::fs::write(day.join("_D5N5068.NEF"), b"raw!").unwrap();
        std::fs::write(day.join("_D5N5068.NEF.xmp"), b"<x/>").unwrap();
        std::fs::write(next.join("_D5N9000+1.NEF"), b"raw").unwrap();

        let c = find_by_date_and_name(
            tmp.path(),
            T,
            "en",
            Date {
                year: 2026,
                month: 8,
                day: 26,
            },
            "_D5N5068.NEF",
            Some(4),
        );
        assert_eq!(c.len(), 2);
        assert!(c[0].path.ends_with("_D5N5068.NEF"));

        // A share that delivered a converted JPEG still finds the RAW by stem.
        let c = find_by_date_and_name(
            tmp.path(),
            T,
            "en",
            Date {
                year: 2026,
                month: 8,
                day: 26,
            },
            "_D5N5068.jpeg",
            None,
        );
        assert!(c[0].path.ends_with("_D5N5068.NEF"));

        // Near midnight: captured on the 26th by the phone's clock, filed on the 27th, with a collision suffix.
        let c = find_by_date_and_name(
            tmp.path(),
            T,
            "en",
            Date {
                year: 2026,
                month: 8,
                day: 26,
            },
            "_D5N9000.NEF",
            None,
        );
        assert!(c[0].path.ends_with("_D5N9000+1.NEF"));
    }

    #[test]
    fn searches_a_folder_library_by_name_without_a_date() {
        let tmp = tempfile::tempdir().unwrap();
        let day = tmp.path().join("2026/08.August/2026-08-26");
        let other = tmp.path().join("2019/05.May/2019-05-02");
        std::fs::create_dir_all(&day).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        std::fs::create_dir_all(tmp.path().join(".hidden")).unwrap();
        std::fs::write(day.join("_D5N5068.JPG"), b"jpeg").unwrap();
        std::fs::write(day.join("_D5N5068.NEF"), b"raw!").unwrap();
        std::fs::write(day.join("_D5N5068.NEF.xmp"), b"<x/>").unwrap();
        std::fs::write(other.join("holiday_D5N5068_edit.NEF"), b"raw").unwrap();
        std::fs::write(tmp.path().join(".hidden/_D5N5068.NEF"), b"raw").unwrap();
        std::fs::write(day.join("notes.txt"), b"text").unwrap();

        // The whole point of this one: a picker can find a photo with no capture date to go on.
        let c = find_by_name(tmp.path(), "_D5N5068", 10);
        let names: Vec<_> = c
            .iter()
            .map(|c| c.path.file_name().unwrap().to_str().unwrap())
            .collect();
        // Exact stem and RAW first, then the exact-stem JPEG, then the substring match. No sidecar, no
        // text file, nothing from a hidden directory.
        assert_eq!(
            names,
            vec!["_D5N5068.NEF", "_D5N5068.JPG", "holiday_D5N5068_edit.NEF"]
        );

        // An extension on the query is a stem too, so pasting a file name works.
        assert_eq!(find_by_name(tmp.path(), "_D5N5068.NEF", 10).len(), 3);
        // Case-insensitive, like Immich's own filename search.
        assert_eq!(find_by_name(tmp.path(), "_d5n5068", 10).len(), 3);
        // The limit is honoured, and the best match survives it.
        let one = find_by_name(tmp.path(), "_D5N5068", 1);
        assert_eq!(one.len(), 1);
        assert!(one[0].path.ends_with("_D5N5068.NEF"));
        // An empty query matches nothing rather than the whole library.
        assert!(find_by_name(tmp.path(), "", 10).is_empty());
        assert!(find_by_name(tmp.path(), "nothing-like-this", 10).is_empty());
    }

    #[test]
    fn a_symlink_loop_does_not_hang_the_walk() {
        let tmp = tempfile::tempdir().unwrap();
        let deep = tmp.path().join("a/b");
        std::fs::create_dir_all(&deep).unwrap();
        std::fs::write(deep.join("IMG_0001.NEF"), b"raw").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(tmp.path(), deep.join("loop")).unwrap();
        let c = find_by_name(tmp.path(), "IMG_0001", 10);
        assert_eq!(c.len(), 1);
    }
}

#[cfg(test)]
mod date_from_unix_tests {
    use super::*;

    /// The same stamps `sidecar_raw`'s `captured_unix` is tested on, read back as dates.
    #[test]
    fn a_timestamp_reads_back_as_the_date_the_camera_wrote() {
        assert_eq!(
            Date::from_unix(0),
            Date {
                year: 1970,
                month: 1,
                day: 1
            }
        );
        assert_eq!(
            Date::from_unix(951_868_800),
            Date {
                year: 2000,
                month: 3,
                day: 1
            }
        );
        assert_eq!(
            Date::from_unix(1_785_349_800),
            Date {
                year: 2026,
                month: 7,
                day: 29
            }
        );
    }

    /// The last second of a day and the first of the next are different days, and a leap day is its own.
    #[test]
    fn day_boundaries_and_a_leap_day() {
        assert_eq!(Date::from_unix(951_868_799).day, 29);
        assert_eq!(Date::from_unix(951_868_799).month, 2);
        assert_eq!(Date::from_unix(951_868_800).day, 1);
    }

    /// Before the epoch, which `div_euclid` handles and a plain division would not.
    #[test]
    fn before_the_epoch() {
        assert_eq!(
            Date::from_unix(-1),
            Date {
                year: 1969,
                month: 12,
                day: 31
            }
        );
    }

    /// ISO weeks across a year boundary, against dates whose week is well known.
    #[test]
    fn iso_weeks() {
        let w = |year, month, day| Date { year, month, day }.iso_week();
        assert_eq!(w(2026, 1, 1), 1, "a Thursday: week 1");
        assert_eq!(w(2021, 1, 1), 53, "a Friday: still 2020's week 53");
        assert_eq!(w(2024, 12, 30), 1, "a Monday: 2025's week 1");
        assert_eq!(w(2026, 11, 6), 45);
        assert_eq!(w(2020, 12, 31), 53);
        assert_eq!(Date::from_unix(0).weekday(), 4, "1970-01-01 was a Thursday");
    }

    /// Layouts that aren't Immich's: an album or event folder under the year, a month written in another language
    /// or case, an abbreviation, a time of day — found by matching folders rather than missed by joining a path.
    #[test]
    fn finds_originals_in_layouts_that_are_not_immichs() {
        let tmp = tempfile::tempdir().unwrap();
        let put = |dir: &str, name: &str| {
            let d = tmp.path().join(dir);
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join(name), b"raw").unwrap();
        };
        put("2019/Holidays in Crete", "DSC_0001.NEF");
        put("2019/.trash", "DSC_0002.NEF");
        put("2019/Mai 2019/Paris", "DSC_0003.NEF");
        put("2018/03.MARS", "DSC_0004.NEF");
        put("2017/sept.", "DSC_0005.NEF");
        put("2016/2016-05-02 14h", "DSC_0006.NEF");
        put("all", "DSC_0007.NEF");
        let date = |year, month, day| Date { year, month, day };
        let find = |template: &str, d: Date, name: &str| {
            find_by_date_and_name(tmp.path(), template, "en", d, name, None)
                .into_iter()
                .map(|c| {
                    c.path
                        .strip_prefix(tmp.path())
                        .unwrap()
                        .to_string_lossy()
                        .into_owned()
                })
                .collect::<Vec<_>>()
        };
        // {{album}} is anything, so the event folder is searched.
        assert_eq!(
            find(
                "{{y}}/{{album}}/{{filename}}",
                date(2019, 7, 4),
                "DSC_0001.NEF"
            ),
            ["2019/Holidays in Crete/DSC_0001.NEF"]
        );
        // Hidden folders are not.
        assert!(
            find(
                "{{y}}/{{album}}/{{filename}}",
                date(2019, 7, 4),
                "DSC_0002.NEF"
            )
            .is_empty()
        );
        // A month in German, with literal text around it, and a nested event.
        assert_eq!(
            find(
                "{{y}}/{{MMMM}} {{y}}/{{album}}/{{filename}}",
                date(2019, 5, 2),
                "DSC_0003.NEF"
            ),
            ["2019/Mai 2019/Paris/DSC_0003.NEF"]
        );
        // A French month in capitals.
        assert_eq!(
            find(
                "{{y}}/{{MM}}.{{MMMM}}/{{filename}}",
                date(2018, 3, 9),
                "DSC_0004.NEF"
            ),
            ["2018/03.MARS/DSC_0004.NEF"]
        );
        // An abbreviation with a full stop.
        assert_eq!(
            find(
                "{{y}}/{{MMM}}/{{filename}}",
                date(2017, 9, 1),
                "DSC_0005.NEF"
            ),
            ["2017/sept./DSC_0005.NEF"]
        );
        // A time of day nobody knows from a capture date.
        assert_eq!(
            find(
                "{{y}}/{{y}}-{{MM}}-{{dd}} {{hh}}h/{{filename}}",
                date(2016, 5, 2),
                "DSC_0006.NEF"
            ),
            ["2016/2016-05-02 14h/DSC_0006.NEF"]
        );
        // Everything in one folder.
        assert_eq!(
            find("all/{{filename}}", date(2001, 1, 1), "DSC_0007.NEF"),
            ["all/DSC_0007.NEF"]
        );
        // The wrong month is still wrong.
        assert!(
            find(
                "{{y}}/{{MM}}.{{MMMM}}/{{filename}}",
                date(2018, 4, 9),
                "DSC_0004.NEF"
            )
            .is_empty()
        );
    }

    #[test]
    fn renders_week_and_other_languages_for_writing() {
        let d = Date {
            year: 2026,
            month: 3,
            day: 4,
        };
        assert_eq!(
            render_dir("{{y}}/W{{WW}}/{{MMMM}}/{{filename}}", d, "de-DE"),
            "2026/W10/März"
        );
        assert_eq!(
            render_dir("{{y}}/{{MMM}}/{{filename}}", Date { month: 2, ..d }, "fr"),
            "2026/fév"
        );
        assert_eq!(render_dir("{{filetype}}/{{filename}}", d, "en"), "IMG");
    }
}
