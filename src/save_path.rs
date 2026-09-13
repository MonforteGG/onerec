use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SavePlan {
    pub folder: PathBuf,
    pub encode: PathBuf,
}

pub(crate) fn save_plan(dialog: &Path, nest: bool) -> SavePlan {
    let folder = dialog
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    if !nest {
        return SavePlan {
            folder,
            encode: dialog.to_path_buf(),
        };
    }
    let Some(name) = dialog.file_name() else {
        return SavePlan {
            folder,
            encode: dialog.to_path_buf(),
        };
    };
    let Some(stem) = dialog.file_stem().filter(|stem| !stem.is_empty()) else {
        return SavePlan {
            folder,
            encode: dialog.to_path_buf(),
        };
    };
    SavePlan {
        folder: folder.clone(),
        encode: folder.join(stem).join(name),
    }
}

impl SavePlan {
    pub(crate) fn needs_overwrite_confirm(&self, dialog: &Path) -> bool {
        self.encode != dialog && self.encode.exists()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nest_off_keeps_the_dialog_path() {
        let dialog = Path::new(r"C:\takes\stem.mp3");
        let plan = save_plan(dialog, false);
        assert_eq!(plan.folder, PathBuf::from(r"C:\takes"));
        assert_eq!(plan.encode, PathBuf::from(r"C:\takes\stem.mp3"));
        assert!(!plan.needs_overwrite_confirm(dialog));
    }

    #[test]
    fn nest_on_rewrites_parent_stem_into_a_take_folder() {
        let dialog = Path::new(r"C:\takes\stem.mp3");
        let plan = save_plan(dialog, true);
        assert_eq!(plan.folder, PathBuf::from(r"C:\takes"));
        assert_eq!(plan.encode, PathBuf::from(r"C:\takes\stem\stem.mp3"));
        assert_ne!(plan.encode, dialog);
    }

    #[test]
    fn missing_file_name_is_a_noop_nest() {
        let dialog = Path::new("");
        let plan = save_plan(dialog, true);
        assert_eq!(plan.encode, dialog);
    }

    #[test]
    fn leftover_flat_file_is_never_the_encode_path() {
        let dir = tempfile::tempdir().unwrap();
        let dialog = dir.path().join("stem.mp3");
        std::fs::write(&dialog, b"flat").unwrap();
        let plan = save_plan(&dialog, true);
        assert_eq!(plan.encode, dir.path().join("stem").join("stem.mp3"));
        assert_ne!(plan.encode, dialog);
        assert!(!plan.needs_overwrite_confirm(&dialog));
        assert!(dialog.exists());
    }

    #[test]
    fn nested_file_exists_asks_to_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        let dialog = dir.path().join("stem.mp3");
        let nested = dir.path().join("stem").join("stem.mp3");
        std::fs::create_dir_all(nested.parent().unwrap()).unwrap();
        std::fs::write(&nested, b"old").unwrap();
        let plan = save_plan(&dialog, true);
        assert!(plan.needs_overwrite_confirm(&dialog));
    }
}
