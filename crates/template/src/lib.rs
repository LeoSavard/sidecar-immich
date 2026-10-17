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

const MONTHS_EN: [&str; 12] =
    ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
const MONTHS_FR: [&str; 12] =
    ["janvier", "février", "mars", "avril", "mai", "juin", "juillet", "août", "septembre", "octobre", "novembre", "décembre"];

fn days_in_month(y: i32, m: u32) -> u32 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 => 29,
        _ => 28,
    }
}

impl Date {
    pub fn add_days(self, delta: i32) -> Date {
        let mut d = self;
        for _ in 0..delta.unsigned_abs() {
            if delta > 0 {
                if d.day < days_in_month(d.year, d.month) {
                    d.day += 1;
                } else if d.month < 12 {
                    d = Date { year: d.year, month: d.month + 1, day: 1 };
                } else {
                    d = Date { year: d.year + 1, month: 1, day: 1 };
                }
            } else if d.day > 1 {
                d.day -= 1;
            } else if d.month > 1 {
                d = Date { year: d.year, month: d.month - 1, day: days_in_month(d.year, d.month - 1) };
            } else {
                d = Date { year: d.year - 1, month: 12, day: 31 };
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
        return (1..=12).contains(&month).then_some(()).and((1..=days_in_month(year, month)).contains(&day).then_some(Date { year, month, day }));
    }
    let mut parts = s.split_whitespace();
    let first = parts.next()?;
    let month = MONTHS_EN.iter().position(|m| first.eq_ignore_ascii_case(m))? as u32 + 1;
    let day: u32 = parts.next()?.trim_end_matches(',').parse().ok()?;
    let year: i32 = parts.next()?.parse().ok()?;
    (day >= 1 && day <= days_in_month(year, month)).then_some(Date { year, month, day })
}

/// Render the directory part of a template (everything before `{{filename}}`).
pub fn render_dir(template: &str, date: Date, month_locale: &str) -> String {
    let months = if month_locale.starts_with("fr") { &MONTHS_FR } else { &MONTHS_EN };
    let dir = template.rsplit_once('/').map_or("", |(d, _)| d);
    dir.replace("{{yyyy}}", &format!("{:04}", date.year))
        .replace("{{yy}}", &format!("{:02}", date.year.rem_euclid(100)))
        .replace("{{y}}", &format!("{:04}", date.year))
        .replace("{{MMMM}}", months[date.month as usize - 1])
        .replace("{{MMM}}", &months[date.month as usize - 1][..3])
        .replace("{{MM}}", &format!("{:02}", date.month))
        .replace("{{M}}", &date.month.to_string())
        .replace("{{dd}}", &format!("{:02}", date.day))
        .replace("{{d}}", &date.day.to_string())
}

pub const RAW_EXTENSIONS: &[&str] =
    &["nef", "nrw", "arw", "srf", "sr2", "raf", "cr2", "cr3", "crw", "dng", "orf", "rw2", "pef", "srw", "x3f", "3fr", "iiq", "erf", "mef", "mos", "rwl"];

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
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| extensions.contains(&e.to_ascii_lowercase().as_str()))
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

/// Look for `name`'s stem in the folders the template gives for `date` and the days either side.
/// Ranking: RAW before other formats, exact name, then matching size; the capture day before its neighbours.
pub fn find_by_date_and_name(root: &Path, template: &str, month_locale: &str, date: Date, name: &str, size: Option<u64>) -> Vec<Candidate> {
    let want_stem = stem_of(name).to_ascii_lowercase();
    let mut out = Vec::new();
    for (day_offset, penalty) in [(0, 0), (-1, 8), (1, 8)] {
        let dir = root.join(render_dir(template, date.add_days(day_offset), month_locale));
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(fname) = path.file_name().and_then(|f| f.to_str()) else { continue };
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
            out.push(Candidate { path, size: md.len(), rank });
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
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        let mut subdirs = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(fname) = path.file_name().and_then(|f| f.to_str()) else { continue };
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
            out.push(Candidate { path, size: md.len(), rank });
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

    const T: &str = "{{y}}/{{MM}}.{{MMMM}}/{{y}}-{{MM}}-{{dd}}/{{filename}}";

    #[test]
    fn renders_the_users_template() {
        let d = Date { year: 2018, month: 3, day: 27 };
        assert_eq!(render_dir(T, d, "en"), "2018/03.March/2018-03-27");
    }

    #[test]
    fn parses_iso_and_long_dates_without_utc_conversion() {
        assert_eq!(parse_capture_date("2018-03-27T22:30:00-04:00"), Some(Date { year: 2018, month: 3, day: 27 }));
        assert_eq!(parse_capture_date("March 27, 2018 at 13:11:29 EDT"), Some(Date { year: 2018, month: 3, day: 27 }));
        assert_eq!(parse_capture_date("2018-02-30T00:00:00Z"), None);
    }

    #[test]
    fn day_arithmetic_crosses_months_and_years() {
        assert_eq!(Date { year: 2024, month: 3, day: 1 }.add_days(-1), Date { year: 2024, month: 2, day: 29 });
        assert_eq!(Date { year: 2025, month: 12, day: 31 }.add_days(1), Date { year: 2026, month: 1, day: 1 });
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

        let c = find_by_date_and_name(tmp.path(), T, "en", Date { year: 2026, month: 8, day: 26 }, "_D5N5068.NEF", Some(4));
        assert_eq!(c.len(), 2);
        assert!(c[0].path.ends_with("_D5N5068.NEF"));

        // A share that delivered a converted JPEG still finds the RAW by stem.
        let c = find_by_date_and_name(tmp.path(), T, "en", Date { year: 2026, month: 8, day: 26 }, "_D5N5068.jpeg", None);
        assert!(c[0].path.ends_with("_D5N5068.NEF"));

        // Near midnight: captured on the 26th by the phone's clock, filed on the 27th, with a collision suffix.
        let c = find_by_date_and_name(tmp.path(), T, "en", Date { year: 2026, month: 8, day: 26 }, "_D5N9000.NEF", None);
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
        let names: Vec<_> = c.iter().map(|c| c.path.file_name().unwrap().to_str().unwrap()).collect();
        // Exact stem and RAW first, then the exact-stem JPEG, then the substring match. No sidecar, no
        // text file, nothing from a hidden directory.
        assert_eq!(names, vec!["_D5N5068.NEF", "_D5N5068.JPG", "holiday_D5N5068_edit.NEF"]);

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
