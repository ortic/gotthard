//! Renames many files by rules, with a preview of every name and its
//! problems before anything is renamed, and a journal to undo it.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use regex::RegexBuilder;
use serde::{Deserialize, Serialize};

use crate::fsx;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Place {
    Start,
    #[default]
    End,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Case {
    #[default]
    Lower,
    Upper,
    /// Every Word Capitalized.
    Title,
    /// Only the first letter.
    Sentence,
}

/// One step of a renaming. The steps work on the name without its
/// extension, except the two for the extension.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Rule {
    Replace {
        find: String,
        with: String,
        regex: bool,
        case_sensitive: bool,
    },
    Insert {
        text: String,
        at: Place,
    },
    /// Removes `count` characters, the first being number `from` (0 is the
    /// first), counted from the end with `from_end`.
    Remove {
        from: usize,
        count: usize,
        from_end: bool,
    },
    Case(Case),
    /// A running number, in the order of the list.
    Number {
        start: u32,
        step: u32,
        digits: usize,
        at: Place,
        separator: String,
    },
    /// The file's time of last change, e.g. `%Y-%m-%d`.
    Date {
        format: String,
        at: Place,
        separator: String,
    },
    /// Removes copy markers (` (2)`, ` - Copy`) and doubled spaces.
    Tidy,
    Extension {
        to: String,
    },
    ExtensionCase {
        upper: bool,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Preview {
    pub path: PathBuf,
    pub old: String,
    pub new: String,
    /// Why the file can't get this name.
    pub problem: Option<String>,
}

impl Preview {
    pub fn changed(&self) -> bool {
        self.old != self.new
    }
}

fn split(name: &str) -> (&str, &str) {
    match name.rfind('.') {
        Some(dot) if dot > 0 => (&name[..dot], &name[dot + 1..]),
        _ => (name, ""),
    }
}

fn place(stem: &str, text: &str, at: Place, separator: &str) -> String {
    match at {
        Place::Start => format!("{text}{separator}{stem}"),
        Place::End => format!("{stem}{separator}{text}"),
    }
}

fn change_case(text: &str, case: Case) -> String {
    match case {
        Case::Lower => text.to_lowercase(),
        Case::Upper => text.to_uppercase(),
        Case::Title => {
            let mut out = String::with_capacity(text.len());
            let mut start = true;
            for c in text.chars() {
                if start {
                    out.extend(c.to_uppercase());
                } else {
                    out.extend(c.to_lowercase());
                }
                start = !c.is_alphanumeric() && c != '\'';
            }
            out
        }
        Case::Sentence => {
            let mut chars = text.chars();
            match chars.next() {
                Some(first) => first
                    .to_uppercase()
                    .chain(chars.as_str().to_lowercase().chars())
                    .collect(),
                None => String::new(),
            }
        }
    }
}

fn valid_date_format(format: &str) -> bool {
    !format.is_empty()
        && !chrono::format::StrftimeItems::new(format)
            .any(|item| matches!(item, chrono::format::Item::Error))
}

/// The name `rules` give the file `name`, the `index`th of the list.
fn new_name(name: &str, index: usize, mtime: i64, rules: &[Rule]) -> Result<String, String> {
    let (stem, ext) = split(name);
    let (mut stem, mut ext) = (stem.to_owned(), ext.to_owned());
    for rule in rules {
        match rule {
            Rule::Replace {
                find,
                with,
                regex,
                case_sensitive,
            } => {
                if find.is_empty() {
                    continue;
                }
                let pattern = if *regex {
                    find.clone()
                } else {
                    regex::escape(find)
                };
                let re = RegexBuilder::new(&pattern)
                    .case_insensitive(!case_sensitive)
                    .build()
                    .map_err(|_| format!("the pattern {find} isn't a valid regular expression"))?;
                stem = if *regex {
                    re.replace_all(&stem, with.as_str()).into_owned()
                } else {
                    re.replace_all(&stem, regex::NoExpand(with)).into_owned()
                };
            }
            Rule::Insert { text, at } => stem = place(&stem, text, *at, ""),
            Rule::Remove {
                from,
                count,
                from_end,
            } => {
                let chars: Vec<char> = stem.chars().collect();
                let len = chars.len();
                let (start, end) = if *from_end {
                    let end = len.saturating_sub(*from);
                    (end.saturating_sub(*count), end)
                } else {
                    ((*from).min(len), from.saturating_add(*count).min(len))
                };
                stem = chars[..start].iter().chain(&chars[end..]).collect();
            }
            Rule::Case(case) => stem = change_case(&stem, *case),
            Rule::Number {
                start,
                step,
                digits,
                at,
                separator,
            } => {
                let n = *start as u64 + index as u64 * *step as u64;
                stem = place(&stem, &format!("{n:0digits$}"), *at, separator);
            }
            Rule::Date {
                format,
                at,
                separator,
            } => {
                if !valid_date_format(format) {
                    return Err(format!("{format} isn't a valid date format"));
                }
                let time: chrono::DateTime<chrono::Local> = fsx::from_millis(mtime).into();
                stem = place(&stem, &time.format(format).to_string(), *at, separator);
            }
            Rule::Tidy => {
                let mut clean = stem.trim().to_owned();
                loop {
                    let shorter = TIDY.replace(&clean, "").trim().to_owned();
                    if shorter.len() == clean.len() {
                        break;
                    }
                    clean = shorter;
                }
                stem = SPACES.replace_all(&clean, " ").into_owned();
            }
            Rule::Extension { to } => ext = to.trim().trim_start_matches('.').to_owned(),
            Rule::ExtensionCase { upper } => {
                ext = if *upper {
                    ext.to_uppercase()
                } else {
                    ext.to_lowercase()
                }
            }
        }
    }
    Ok(if ext.is_empty() {
        stem
    } else {
        format!("{stem}.{ext}")
    })
}

static TIDY: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r"(?i)(\s*\(\d+\)|\s+-\s+(copy|kopie|copie)(\s*\(\d+\))?)$").expect("valid")
});
static SPACES: std::sync::LazyLock<regex::Regex> =
    std::sync::LazyLock::new(|| regex::Regex::new(r"\s{2,}").expect("valid"));

/// Why Windows wouldn't take `name` as a file name.
pub fn name_problem(name: &str) -> Option<String> {
    const RESERVED: [&str; 22] = [
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
        "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    if name.is_empty() || split(name).0.trim().is_empty() {
        return Some("the name is empty".into());
    }
    if let Some(c) = name.chars().find(|c| {
        matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') || c.is_control()
    }) {
        return Some(format!("{c:?} isn't allowed in file names"));
    }
    if name.ends_with(['.', ' ']) {
        return Some("a name can't end with a dot or a space".into());
    }
    if name.starts_with(' ') {
        return Some("a name can't start with a space".into());
    }
    let base = name.split('.').next().unwrap_or(name).trim_end();
    if RESERVED.iter().any(|r| r.eq_ignore_ascii_case(base)) {
        return Some(format!("{base} is a name Windows reserves"));
    }
    if name.encode_utf16().count() > 255 {
        return Some("the name is longer than 255 characters".into());
    }
    None
}

/// The names `rules` give `files`, in their order, with what stands in the
/// way of each. Changes nothing.
pub fn preview(files: &[PathBuf], rules: &[Rule]) -> Vec<Preview> {
    let mut previews: Vec<Preview> = files
        .iter()
        .enumerate()
        .map(|(index, path)| {
            let old = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let mtime = fsx::stat(path)
                .ok()
                .flatten()
                .map_or(0, |(stamp, _)| stamp.mtime);
            let (new, problem) = match new_name(&old, index, mtime, rules) {
                Ok(new) => {
                    let problem = name_problem(&new);
                    (new, problem)
                }
                Err(problem) => (old.clone(), Some(problem)),
            };
            Preview {
                path: path.clone(),
                old,
                new,
                problem,
            }
        })
        .collect();

    // Two files can't get one name, and a name can't be taken by a file
    // that keeps it. Windows doesn't tell upper from lower case.
    let target = |p: &Preview| p.path.with_file_name(&p.new);
    let fold = |path: &Path| path.to_string_lossy().to_lowercase();
    let sources: HashSet<String> = previews.iter().map(|p| fold(&p.path)).collect();
    let mut targets: HashMap<String, usize> = HashMap::new();
    for p in &previews {
        *targets.entry(fold(&target(p))).or_default() += 1;
    }
    for p in &mut previews {
        if p.problem.is_some() {
            continue;
        }
        let to = target(p);
        if targets[&fold(&to)] > 1 {
            p.problem = Some("another file of the list gets the same name".into());
        } else if p.changed() && !sources.contains(&fold(&to)) && fs::symlink_metadata(&to).is_ok()
        {
            p.problem = Some("a file with this name exists".into());
        }
    }
    previews
}

/// The renames that were done, to undo them.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Journal {
    /// From, to; in the order they were done.
    pub renames: Vec<(PathBuf, PathBuf)>,
}

/// Renames the files of the preview. Refuses when a name has a problem;
/// when a rename fails, the ones before it are taken back.
pub fn apply(previews: &[Preview]) -> Result<Journal> {
    if let Some(p) = previews.iter().find(|p| p.problem.is_some()) {
        bail!(
            "{} can't be renamed to {}: {}",
            p.old,
            p.new,
            p.problem.as_deref().unwrap_or_default()
        );
    }
    let changes: Vec<&Preview> = previews.iter().filter(|p| p.changed()).collect();
    let mut journal = Journal::default();
    let result = (|| -> Result<()> {
        // A name another file of the list still has (a swap, a chain, or
        // only another case): everything first moves to a name nobody has.
        let (mut parked, mut direct) = (Vec::new(), Vec::new());
        for (n, p) in changes.iter().enumerate() {
            let to = p.path.with_file_name(&p.new);
            if fs::symlink_metadata(&to).is_ok() {
                let tmp = p
                    .path
                    .with_file_name(format!(".gotthard-rename-{n}-{}", std::process::id()));
                step(&mut journal, &p.path, &tmp)?;
                parked.push((tmp, to));
            } else {
                direct.push((p.path.clone(), to));
            }
        }
        for (from, to) in direct.into_iter().chain(parked) {
            if fs::symlink_metadata(&to).is_ok() {
                bail!("{} exists", to.display());
            }
            step(&mut journal, &from, &to)?;
        }
        Ok(())
    })();
    if let Err(e) = result {
        let _ = undo(&journal);
        return Err(e);
    }
    Ok(journal)
}

fn step(journal: &mut Journal, from: &Path, to: &Path) -> Result<()> {
    fs::rename(from, to)
        .with_context(|| format!("can't rename {} to {}", from.display(), to.display()))?;
    journal.renames.push((from.to_path_buf(), to.to_path_buf()));
    Ok(())
}

/// Takes the renames of a journal back, the last first. Returns how many
/// couldn't be taken back (the file is gone, or its old name is taken).
pub fn undo(journal: &Journal) -> usize {
    journal
        .renames
        .iter()
        .rev()
        .filter(|(from, to)| fs::symlink_metadata(from).is_ok() || fs::rename(to, from).is_err())
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(rules: &[Rule], files: &[&str]) -> Vec<String> {
        files
            .iter()
            .enumerate()
            .map(|(i, name)| new_name(name, i, 0, rules).unwrap())
            .collect()
    }

    #[test]
    fn rules() {
        let replace = |find: &str, with: &str, regex| Rule::Replace {
            find: find.into(),
            with: with.into(),
            regex,
            case_sensitive: false,
        };
        assert_eq!(
            names(&[replace("img_", "Holiday ", false)], &["IMG_0012.JPG"]),
            ["Holiday 0012.JPG"]
        );
        assert_eq!(
            names(
                &[replace(r"(\d+)-(\d+)", "$2-$1", true)],
                &["12-2024 a.b.txt"]
            ),
            ["2024-12 a.b.txt"]
        );
        // Not a regular expression: taken literally, both ways.
        assert_eq!(
            names(&[replace("(1)", "$1", false)], &["a (1).txt"]),
            ["a $1.txt"]
        );
        assert_eq!(
            names(
                &[
                    Rule::Remove {
                        from: 0,
                        count: 4,
                        from_end: false
                    },
                    Rule::Remove {
                        from: 0,
                        count: 2,
                        from_end: true
                    },
                    Rule::Case(Case::Title),
                    Rule::Number {
                        start: 1,
                        step: 1,
                        digits: 3,
                        at: Place::Start,
                        separator: " - ".into()
                    },
                    Rule::ExtensionCase { upper: false },
                ],
                &["IMG_summer in the alps_x.JPG", "IMG_o'neil's bar_x.JPG"]
            ),
            ["001 - Summer In The Alps.jpg", "002 - O'neil's Bar.jpg"]
        );
        assert_eq!(
            names(
                &[Rule::Tidy, Rule::Extension { to: ".md".into() }],
                &["Notes  final - Copy (2).txt"]
            ),
            ["Notes final.md"]
        );
        assert_eq!(change_case("hELLO wORLD", Case::Sentence), "Hello world");
        assert!(new_name("a", 0, 0, &[replace("(", "", true)]).is_err());
        let date = Rule::Date {
            format: "%Y-%m-%d".into(),
            at: Place::Start,
            separator: " ".into(),
        };
        let named = new_name("a.txt", 0, 86_400_000 * 366, &[date]).unwrap();
        assert!(named.starts_with("1971-01-0"), "{named}");
        let bad = Rule::Date {
            format: "%Q".into(),
            at: Place::Start,
            separator: String::new(),
        };
        assert!(new_name("a.txt", 0, 0, &[bad]).is_err());
    }

    #[test]
    fn names_windows_refuses() {
        assert!(name_problem("a:b.txt").is_some());
        assert!(name_problem("con.txt").is_some());
        assert!(name_problem("LPT1").is_some());
        assert!(name_problem("trailing.").is_some());
        assert!(name_problem(".txt").is_none());
        assert!(name_problem(" .txt").is_some());
        assert!(name_problem("Console.txt").is_none());
    }

    #[test]
    fn swaps_names_and_undoes() {
        let dir = tempfile::tempdir().unwrap();
        let file = |name: &str| dir.path().join(name);
        fs::write(file("a.txt"), "a").unwrap();
        fs::write(file("b.txt"), "b").unwrap();
        fs::write(file("c.txt"), "c").unwrap();
        let mut previews = preview(&[file("a.txt"), file("b.txt")], &[]);
        previews[0].new = "b.txt".into();
        previews[1].new = "a.txt".into();
        let journal = apply(&previews).unwrap();
        assert_eq!(fs::read_to_string(file("a.txt")).unwrap(), "b");
        assert_eq!(fs::read_to_string(file("b.txt")).unwrap(), "a");
        assert_eq!(undo(&journal), 0);
        assert_eq!(fs::read_to_string(file("a.txt")).unwrap(), "a");
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 3);

        // Names that collide are found before anything is renamed.
        let rule = Rule::Replace {
            find: "[ab]".into(),
            with: "c".into(),
            regex: true,
            case_sensitive: true,
        };
        let previews = preview(&[file("a.txt"), file("b.txt")], std::slice::from_ref(&rule));
        assert!(previews.iter().all(|p| p.problem.is_some()));
        assert!(apply(&previews).is_err());
        let previews = preview(&[file("a.txt")], &[rule]);
        assert_eq!(
            previews[0].problem.as_deref(),
            Some("a file with this name exists")
        );
    }
}
