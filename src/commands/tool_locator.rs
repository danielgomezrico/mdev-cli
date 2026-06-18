use std::path::PathBuf;

use crate::runner::Runner;

fn find(runner: &dyn Runner, executable: &str, env_var: &str, subpath: &[&str]) -> Option<String> {
    if let Some(path) = runner.which(executable) {
        return Some(path);
    }
    if let Ok(root) = std::env::var(env_var) {
        let mut candidate = PathBuf::from(root);
        for component in subpath {
            candidate = candidate.join(component);
        }
        candidate = candidate.join(executable);
        if candidate.exists() {
            return Some(candidate.to_string_lossy().to_string());
        }
    }
    None
}

pub fn keytool(runner: &dyn Runner) -> Option<String> {
    find(runner, "keytool", "JAVA_HOME", &["bin"])
}

pub fn adb(runner: &dyn Runner) -> Option<String> {
    find(runner, "adb", "ANDROID_HOME", &["platform-tools"])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::RunResult;
    use std::fs;
    use tempfile::TempDir;

    struct MockRunner {
        which_result: Option<String>,
    }

    impl Runner for MockRunner {
        fn run(&self, _executable: &str, _args: &[&str], _working_dir: Option<&str>) -> RunResult {
            RunResult::new(0, String::new(), String::new())
        }

        fn which(&self, _executable: &str) -> Option<String> {
            self.which_result.clone()
        }
    }

    #[test]
    fn which_found_returns_that_path() {
        let runner = MockRunner {
            which_result: Some("/usr/bin/keytool".to_string()),
        };
        assert_eq!(keytool(&runner), Some("/usr/bin/keytool".to_string()));
    }

    #[test]
    fn which_not_found_env_absent_returns_none() {
        let runner = MockRunner { which_result: None };
        // Use a clearly-unused env var name to avoid collisions with real env
        let result = find(&runner, "keytool", "MDEV_TEST_NONEXISTENT_HOME", &["bin"]);
        assert_eq!(result, None);
    }

    #[test]
    fn which_not_found_env_set_with_existing_file_returns_path() {
        let tmp = TempDir::new().unwrap();
        let bin_dir = tmp.path().join("bin");
        fs::create_dir_all(&bin_dir).unwrap();
        let keytool_path = bin_dir.join("keytool");
        fs::write(&keytool_path, "").unwrap();

        let env_var = "MDEV_TEST_JAVA_HOME_UNIQUE";
        std::env::set_var(env_var, tmp.path());

        let runner = MockRunner { which_result: None };
        let result = find(&runner, "keytool", env_var, &["bin"]);

        std::env::remove_var(env_var);

        assert_eq!(
            result,
            Some(keytool_path.to_string_lossy().to_string())
        );
    }

    // GUARD: env set but candidate file does not exist → None (`.exists()` guard)
    #[test]
    fn find_env_set_file_missing_returns_none() {
        let tmp = TempDir::new().unwrap();
        // Do NOT create the file — the directory exists, the file does not
        let env_var = "MDEV_TEST_FIND_ENV_MISSING_FILE";
        std::env::set_var(env_var, tmp.path());

        let runner = MockRunner { which_result: None };
        let result = find(&runner, "keytool", env_var, &["bin"]);

        std::env::remove_var(env_var);

        assert_eq!(result, None);
    }

    // GUARD: adb public function — happy path via $ANDROID_HOME/platform-tools/adb
    #[test]
    fn adb_env_set_existing_file_returns_path() {
        let tmp = TempDir::new().unwrap();
        let pt_dir = tmp.path().join("platform-tools");
        fs::create_dir_all(&pt_dir).unwrap();
        let adb_path = pt_dir.join("adb");
        fs::write(&adb_path, "").unwrap();

        let env_var = "ANDROID_HOME";
        let prev = std::env::var(env_var).ok();
        std::env::set_var(env_var, tmp.path());

        let runner = MockRunner { which_result: None };
        let result = adb(&runner);

        match prev {
            Some(v) => std::env::set_var(env_var, v),
            None => std::env::remove_var(env_var),
        }

        assert_eq!(result, Some(adb_path.to_string_lossy().to_string()));
    }

    // GUARD: multi-segment subpath (&["platform-tools", "x"]) builds the correct joined path
    #[test]
    fn find_multi_segment_subpath_builds_correct_path() {
        let tmp = TempDir::new().unwrap();
        let nested = tmp.path().join("platform-tools").join("x");
        fs::create_dir_all(&nested).unwrap();
        let exe_path = nested.join("adb");
        fs::write(&exe_path, "").unwrap();

        let env_var = "MDEV_TEST_FIND_MULTISEG";
        std::env::set_var(env_var, tmp.path());

        let runner = MockRunner { which_result: None };
        let result = find(&runner, "adb", env_var, &["platform-tools", "x"]);

        std::env::remove_var(env_var);

        assert_eq!(result, Some(exe_path.to_string_lossy().to_string()));
    }

    // GUARD: env var set to "" produces a relative path; `.exists()` returns false → None, no panic
    #[test]
    fn find_env_empty_string_no_panic() {
        let env_var = "MDEV_TEST_FIND_EMPTY_ROOT";
        std::env::set_var(env_var, "");

        let runner = MockRunner { which_result: None };
        // Should not panic; the relative candidate path will not exist
        let result = find(&runner, "keytool", env_var, &["bin"]);

        std::env::remove_var(env_var);

        assert_eq!(result, None);
    }

    // GUARD: which returns Some("") (empty string) → returned verbatim, no normalization
    #[test]
    fn find_which_empty_string_returned_verbatim() {
        let runner = MockRunner {
            which_result: Some(String::new()),
        };
        let result = keytool(&runner);
        assert_eq!(result, Some(String::new()));
    }
}
