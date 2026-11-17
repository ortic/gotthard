//! Drives the real app, headless, through its main flows; with
//! `GOTTHARD_SCREENSHOT_DIR` set it also renders each step (wgpu) into PNGs
//! for visual review:
//!
//! ```text
//! GOTTHARD_SCREENSHOT_DIR=/tmp/shots cargo test -p gotthard-app screenshots
//! ```

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use gotthard_config::{Backup, Pair, Paths, Settings};
use gotthard_core::master;

use crate::app::{App, View};

fn out_dir() -> Option<PathBuf> {
    std::env::var_os("GOTTHARD_SCREENSHOT_DIR").map(PathBuf::from)
}

/// Rendering tests run one at a time: several wgpu renderers in one
/// process crash now and then.
static RENDER: std::sync::Mutex<()> = std::sync::Mutex::new(());

struct Scene {
    harness: Harness<'static, App>,
    dir: tempfile::TempDir,
    _lock: Option<std::sync::MutexGuard<'static, ()>>,
}

fn write(root: &Path, rel: &str, content: &str, age_hours: u64) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, content).unwrap();
    fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(SystemTime::now() - Duration::from_secs(age_hours * 3600))
        .unwrap();
}

/// A master on a "disk" and a folder of this computer, synchronized once
/// and changed on both sides since.
fn scene(dark: bool) -> Scene {
    let dir = tempfile::tempdir().unwrap();
    let master = dir.path().join("disk/Master/Photos");
    let device = dir.path().join("pc/Pictures");
    for (rel, content) in [
        ("2024/Zermatt/IMG_0101.jpg", "matterhorn"),
        ("2024/Zermatt/IMG_0102.jpg", "gornergrat"),
        ("2024/Ticino/IMG_0201.jpg", "bellinzona"),
        ("2025/Andermatt/IMG_0301.jpg", "teufelsbruecke"),
        ("2025/Andermatt/notes.txt", "first draft"),
        ("albums.txt", "2024, 2025"),
    ] {
        write(&master, rel, content, 200);
        write(&device, rel, content, 200);
    }
    let marker = master::create(&master, "Photos").unwrap();
    let paths = Paths::in_dir(&dir.path().join("settings"));
    let pair = Pair {
        name: "Photos".into(),
        master_id: marker.id.clone(),
        master_path: master.clone(),
        device_path: device.clone(),
        ..Pair::default()
    };
    // The state of a first run with everything equal.
    let mut state = gotthard_core::sync::State::new(&marker.id, &device);
    let plan = gotthard_core::sync::compare(
        &master,
        &device,
        None,
        &Default::default(),
        &Default::default(),
    )
    .unwrap();
    gotthard_core::sync::execute(&plan, &mut state, &Default::default(), &Default::default())
        .unwrap();
    state.save(&paths.state_file(&pair.id)).unwrap();

    // Since then:
    write(
        &master,
        "2025/Gotthard/IMG_0401.jpg",
        "the pass, seen from the south",
        5,
    );
    write(&master, "2025/Gotthard/IMG_0402.jpg", "tremola", 5);
    write(&device, "2025/Andermatt/IMG_0302.jpg", "schoellenen", 3);
    write(&device, "albums.txt", "2024, 2025, 2026", 2);
    write(
        &master,
        "2025/Andermatt/notes.txt",
        "edited on the laptop",
        4,
    );
    write(
        &device,
        "2025/Andermatt/notes.txt",
        "edited on the desktop",
        1,
    );
    fs::remove_file(master.join("2024/Ticino/IMG_0201.jpg")).unwrap();

    let settings = Settings {
        pairs: vec![
            pair,
            Pair {
                name: "Documents".into(),
                master_path: dir.path().join("disk/Master/Documents"),
                device_path: dir.path().join("pc/Documents"),
                master_id: "0123456789abcdef".into(),
                ..Pair::default()
            },
        ],
        backups: vec![Backup {
            name: "Pictures to the NAS disk".into(),
            source: device,
            destination: dir.path().join("backup/Pictures"),
            ..Backup::default()
        }],
        ..Settings::default()
    };
    settings.save(&paths).unwrap();

    let render = out_dir().is_some();
    let lock = render.then(|| RENDER.lock().unwrap_or_else(|e| e.into_inner()));
    let builder = Harness::builder()
        .with_size(egui::vec2(1280.0, 800.0))
        .with_pixels_per_point(1.5);
    let builder = if render { builder.wgpu() } else { builder };
    let harness = builder.build_eframe(|cc| App::new(cc, Some(paths)));
    harness.ctx.set_theme(if dark {
        egui::Theme::Dark
    } else {
        egui::Theme::Light
    });
    Scene {
        harness,
        dir,
        _lock: lock,
    }
}

impl Scene {
    fn path(&self, rel: &str) -> PathBuf {
        self.dir.path().join(rel)
    }

    fn read(&self, rel: &str) -> Option<String> {
        fs::read_to_string(self.path(rel)).ok()
    }

    fn save(&mut self, name: &str) {
        self.harness.run_steps(4);
        let Some(dir) = out_dir() else { return };
        fs::create_dir_all(&dir).unwrap();
        let image = self.harness.render().expect("render");
        let path = dir.join(format!("{name}.png"));
        image.save(&path).unwrap();
        eprintln!("wrote {}", path.display());
    }

    /// Steps the app until the running operation is done.
    fn wait(&mut self) {
        let start = Instant::now();
        self.harness.step();
        while self.harness.state().busy() {
            assert!(start.elapsed() < Duration::from_secs(30), "still busy");
            self.harness.step();
            std::thread::sleep(Duration::from_millis(10));
        }
        self.harness.run_steps(3);
    }

    fn click(&mut self, label: &str) {
        self.harness.get_by_label(label).click();
        self.harness.run_steps(2);
    }

    /// Changes the app's state as typing or choosing would.
    fn edit(&mut self, change: impl FnOnce(&mut App)) {
        change(self.harness.state_mut());
        self.harness.run_steps(2);
    }

    fn go(&mut self, view: View) {
        self.harness.state_mut().view = view;
        self.harness.run_steps(2);
    }
}

#[test]
fn synchronize_a_pair() {
    for dark in [true, false] {
        let suffix = if dark { "dark" } else { "light" };
        let mut s = scene(dark);
        s.save(&format!("01-sync-start-{suffix}"));
        s.click("Compare");
        s.wait();
        let summary = s
            .harness
            .state()
            .sync
            .panel
            .plan
            .as_ref()
            .unwrap()
            .summary();
        assert_eq!(summary.conflicts, 1, "{summary:?}");
        assert_eq!(summary.to_device, 3);
        assert_eq!(summary.to_master, 2);
        assert_eq!(summary.delete_device, 1);
        s.save(&format!("02-sync-plan-{suffix}"));

        s.click("Master wins");
        s.save(&format!("03-sync-decided-{suffix}"));
        s.click("Synchronize (7 changes)");
        s.wait();
        let report = s.harness.state().sync.panel.report.clone().unwrap();
        assert!(report.clean(), "{report:?}");
        s.save(&format!("04-sync-done-{suffix}"));
        assert_eq!(
            s.read("pc/Pictures/2025/Andermatt/notes.txt").as_deref(),
            Some("edited on the laptop")
        );
        assert_eq!(
            s.read("disk/Master/Photos/albums.txt").as_deref(),
            Some("2024, 2025, 2026")
        );
        assert!(!s.path("pc/Pictures/2024/Ticino/IMG_0201.jpg").exists());

        // Nothing left to do, and what was replaced can be restored.
        s.click("Compare");
        s.wait();
        assert_eq!(
            s.harness
                .state()
                .sync
                .panel
                .plan
                .as_ref()
                .unwrap()
                .summary()
                .changes(),
            0
        );
        s.go(View::Vault);
        let root = s.path("pc/Pictures");
        s.edit(|app| app.vault.root = root);
        s.wait();
        assert_eq!(s.harness.state().vault.sessions[0].files.len(), 2);
        s.save(&format!("05-safety-copies-{suffix}"));
    }
}

#[test]
fn back_up_a_folder() {
    let mut s = scene(true);
    s.go(View::Backup);
    s.click("Compare");
    s.wait();
    s.save("06-backup-plan-dark");
    s.click("Back up (12 changes)");
    s.wait();
    assert!(
        s.harness
            .state()
            .backup
            .panel
            .report
            .as_ref()
            .unwrap()
            .clean()
    );
    assert_eq!(
        s.read("backup/Pictures/albums.txt").as_deref(),
        Some("2024, 2025, 2026")
    );
    s.click("Check the backup for damage");
    s.wait();
    let report = s.harness.state().backup.panel.integrity.clone().unwrap();
    assert_eq!((report.new.len(), report.damaged.len()), (7, 0));
    s.save("07-backup-checked-dark");
}

#[test]
fn the_file_tools() {
    let mut s = scene(false);
    let pc = s.path("pc/Pictures");
    write(&pc, "2024/Zermatt/IMG_0101 (1).jpg", "matterhorn", 100);
    write(
        &pc,
        "2024/Zermatt/IMG_0101 - Copy.jpg",
        "matterhorn, edited",
        90,
    );
    write(&pc, "Unsorted/matterhorn.jpg", "matterhorn", 80);

    s.go(View::Duplicates);
    s.edit(|app| {
        app.dupes.folders = vec![pc.clone()];
        app.dupes.min_kb = 0;
    });
    s.click("Search");
    s.wait();
    assert_eq!(
        s.harness
            .state()
            .dupes
            .result
            .as_ref()
            .unwrap()
            .groups
            .len(),
        1
    );
    s.click("the oldest");
    assert_eq!(s.harness.state().dupes.marked.len(), 2);
    s.save("08-duplicates-light");

    s.go(View::Similar);
    s.edit(|app| app.similar.folders = vec![pc.clone()]);
    s.click("Search");
    s.wait();
    assert_eq!(s.harness.state().similar.groups.as_ref().unwrap().len(), 1);
    s.save("09-similar-light");

    s.click("Rename these…");
    assert_eq!(s.harness.state().view, View::Rename);
    s.edit(|app| {
        use gotthard_core::rename::{Place, Rule};
        app.rename.rules = vec![
            Rule::Replace {
                find: "IMG_".into(),
                with: "Zermatt ".into(),
                regex: false,
                case_sensitive: false,
            },
            Rule::Number {
                start: 1,
                step: 1,
                digits: 2,
                at: Place::End,
                separator: " - ".into(),
            },
        ];
    });
    s.save("10-rename-light");
    s.click("Rename 3 files");
    assert!(pc.join("2024/Zermatt/Zermatt 0101 (1) - 01.jpg").exists());
    s.click("Undo the last rename");
    assert!(pc.join("2024/Zermatt/IMG_0101 (1).jpg").exists());

    s.go(View::Merge);
    let (sources, target) = (
        vec![pc.join("2024"), s.path("disk/Master/Photos/2025")],
        s.path("pc/Merged"),
    );
    s.edit(|app| {
        app.merge.sources = sources;
        app.merge.target = target;
    });
    s.click("Compare");
    s.wait();
    s.save("11-merge-light");
    assert_eq!(
        s.harness.state().merge.plan.as_ref().unwrap().to_copy().0,
        9
    );

    s.go(View::Browse);
    let (left, right) = (pc.join("2024/Zermatt"), s.path("disk/Master/Photos"));
    s.edit(|app| {
        app.browse.panes[0].dir = left;
        app.browse.panes[1].dir = right;
    });
    s.save("12-browse-light");
    s.go(View::Settings);
    s.save("13-settings-light");
}
