//! Finds files with similar names: `Report.docx`, `Report (1).docx`,
//! `Report - Copy.docx`, `report_final_v2.docx`.

use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;
use std::sync::LazyLock;

use anyhow::Result;
use rayon::prelude::*;
use regex::Regex;

use crate::Progress;
use crate::fsx::join_rel;
use crate::scan::{self, Filter};

#[derive(Clone, Debug)]
pub struct SimilarOptions {
    /// How alike two cleaned names must be, 0.5..=1.0; 1.0 groups only names
    /// that are the same once copy and version markers are removed.
    pub threshold: f64,
    /// Only files of the same extension are similar.
    pub same_extension: bool,
    pub filter: Filter,
}

impl Default for SimilarOptions {
    fn default() -> Self {
        Self {
            threshold: 1.0,
            same_extension: true,
            filter: Filter::standard(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct SimilarFile {
    pub path: PathBuf,
    pub size: u64,
    pub mtime: i64,
}

#[derive(Clone, Debug)]
pub struct SimilarGroup {
    /// The cleaned name the files share.
    pub name: String,
    pub files: Vec<SimilarFile>,
}

static MARKERS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?x)(
            \s*\(\d+\)                                   # (2)
          | [\s_\-]+(copy|kopie|copie)(\s*\(\d+\)|\s*\d+)?   # - Copy, - Kopie (2)
          | [\s_\-]+(final|neu|new|old|alt|backup|bak|draft|entwurf)
          | [\s_\-]+v\d+                                 # _v2
        )$",
    )
    .expect("valid")
});
static PREFIXES: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(copy of|kopie von|copie de)\s+").expect("valid"));
static SEPARATORS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[\s_\-.]+").expect("valid"));

/// A file name without what copies and versions add to it, in lower case,
/// and its extension.
pub fn clean_name(name: &str) -> (String, String) {
    let lower = name.to_lowercase();
    let (stem, ext) = match lower.rfind('.') {
        Some(dot) if dot > 0 => (&lower[..dot], &lower[dot + 1..]),
        _ => (lower.as_str(), ""),
    };
    let mut clean = stem.trim().to_owned();
    loop {
        let shorter = MARKERS.replace(&clean, "");
        let shorter = PREFIXES.replace(&shorter, "");
        if shorter.len() == clean.len() {
            break;
        }
        clean = shorter.trim().to_owned();
    }
    let clean = SEPARATORS.replace_all(&clean, " ").trim().to_owned();
    if clean.is_empty() {
        (stem.to_owned(), ext.to_owned())
    } else {
        (clean, ext.to_owned())
    }
}

/// Names that differ only in their numbers are a series (`IMG_0012`,
/// `IMG_0013`), not versions of one file.
fn series(a: &str, b: &str) -> bool {
    let letters = |s: &str| -> String { s.chars().filter(|c| !c.is_ascii_digit()).collect() };
    a != b && a.chars().any(|c| c.is_ascii_digit()) && letters(a) == letters(b)
}

fn alike(a: &str, b: &str, threshold: f64) -> bool {
    let (la, lb) = (a.chars().count(), b.chars().count());
    // Short names are alike by accident.
    if la.min(lb) < 4 || series(a, b) {
        return false;
    }
    // The lengths alone can rule it out.
    if (la.abs_diff(lb) as f64) / (la.max(lb) as f64) > 1.0 - threshold {
        return false;
    }
    strsim::normalized_levenshtein(a, b) >= threshold
}

/// Beyond this many different names of one extension only neighbours in
/// alphabetical order are compared.
const ALL_PAIRS: usize = 4000;
const WINDOW: usize = 40;

pub fn find(
    roots: &[PathBuf],
    options: &SimilarOptions,
    progress: &Progress,
) -> Result<Vec<SimilarGroup>> {
    progress.phase("Reading the folders", 0, 0);
    // Extension (or none when it doesn't matter) → cleaned name → files.
    let mut buckets: BTreeMap<String, BTreeMap<String, Vec<SimilarFile>>> = BTreeMap::new();
    let mut seen = HashSet::new();
    for root in roots {
        let tree = scan::scan(root, &options.filter, progress)?;
        for entry in tree.files() {
            let path = join_rel(root, &entry.rel);
            if !seen.insert(path.clone()) {
                continue;
            }
            let name = entry.rel.rsplit('/').next().unwrap_or(&entry.rel);
            let (clean, ext) = clean_name(name);
            let bucket = if options.same_extension {
                ext
            } else {
                String::new()
            };
            buckets
                .entry(bucket)
                .or_default()
                .entry(clean)
                .or_default()
                .push(SimilarFile {
                    path,
                    size: entry.stamp.size,
                    mtime: entry.stamp.mtime,
                });
        }
    }

    progress.phase("Comparing names", buckets.len() as u64, 0);
    let threshold = options.threshold.clamp(0.5, 1.0);
    let mut groups = Vec::new();
    for names in buckets.into_values() {
        progress.check()?;
        progress.item();
        let keys: Vec<&String> = names.keys().collect();
        // Union-find over the names.
        let mut parent: Vec<usize> = (0..keys.len()).collect();
        fn root_of(parent: &mut [usize], mut i: usize) -> usize {
            while parent[i] != i {
                parent[i] = parent[parent[i]];
                i = parent[i];
            }
            i
        }
        if threshold < 1.0 {
            let pairs: Vec<(usize, usize)> = (0..keys.len())
                .into_par_iter()
                .flat_map_iter(|i| {
                    let end = if keys.len() <= ALL_PAIRS {
                        keys.len()
                    } else {
                        (i + 1 + WINDOW).min(keys.len())
                    };
                    let keys = &keys;
                    (i + 1..end)
                        .filter(move |&j| alike(keys[i], keys[j], threshold))
                        .map(move |j| (i, j))
                })
                .collect();
            for (i, j) in pairs {
                let (a, b) = (root_of(&mut parent, i), root_of(&mut parent, j));
                parent[b.max(a)] = a.min(b);
            }
        }
        let mut merged: BTreeMap<usize, SimilarGroup> = BTreeMap::new();
        for (i, (name, files)) in names.iter().enumerate() {
            let group = root_of(&mut parent, i);
            merged
                .entry(group)
                .or_insert_with(|| SimilarGroup {
                    name: name.clone(),
                    files: Vec::new(),
                })
                .files
                .extend(files.iter().cloned());
        }
        groups.extend(merged.into_values().filter(|g| g.files.len() > 1));
    }
    for group in &mut groups {
        group.files.sort_by(|a, b| a.path.cmp(&b.path));
    }
    groups.sort_by(|a, b| {
        a.name
            .cmp(&b.name)
            .then(a.files[0].path.cmp(&b.files[0].path))
    });
    Ok(groups)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn cleans_copy_and_version_markers() {
        for (name, clean) in [
            ("Report.docx", "report"),
            ("Report (1).docx", "report"),
            ("Report - Copy.docx", "report"),
            ("Report - Copy (2).docx", "report"),
            ("Report - Kopie.docx", "report"),
            ("Copy of Report.docx", "report"),
            ("report_final_v2.docx", "report"),
            ("Report-final.DOCX", "report"),
            ("Annual_Report-2024.docx", "annual report 2024"),
            ("IMG_0012.jpg", "img 0012"),
            ("(1).txt", "(1)"),
        ] {
            assert_eq!(clean_name(name).0, clean, "{name}");
        }
        assert_eq!(clean_name("a.tar.GZ").1, "gz");
        assert_eq!(clean_name(".profile"), ("profile".into(), "".into()));
    }

    #[test]
    fn groups_versions_but_not_series() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("old")).unwrap();
        for name in [
            "Report.docx",
            "Report (1).docx",
            "old/report_final.docx",
            "Report.pdf",
            "IMG_0012.jpg",
            "IMG_0013.jpg",
            "Holiday photos 2024.zip",
            "Holiday fotos 2024.zip",
        ] {
            fs::write(root.join(name), "x").unwrap();
        }
        let roots = [root.to_path_buf()];
        let mut options = SimilarOptions::default();
        let groups = find(&roots, &options, &Progress::default()).unwrap();
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].name, "report");
        assert_eq!(groups[0].files.len(), 3);

        options.threshold = 0.85;
        let groups = find(&roots, &options, &Progress::default()).unwrap();
        let names: Vec<&str> = groups.iter().map(|g| g.name.as_str()).collect();
        assert_eq!(names, ["holiday fotos 2024", "report"]);

        options.same_extension = false;
        let groups = find(&roots, &options, &Progress::default()).unwrap();
        assert_eq!(groups[1].files.len(), 4);
    }
}
