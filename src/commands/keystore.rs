use clap::Args;
use colored::Colorize;
use std::path::PathBuf;

use crate::app_detector::AppDetector;
use crate::logger::Logger;
use crate::models::ProjectType;
use crate::runner::Runner;

#[derive(Args, Debug)]
pub struct KeystoreArgs {
    /// Verbose output
    #[arg(short = 'v', long)]
    pub verbose: bool,
}

pub fn run(args: &KeystoreArgs, runner: &dyn Runner) -> i32 {
    let logger = Logger::new();

    // Find keytool
    let keytool = find_keytool(runner);
    if keytool.is_none() {
        logger.err("keytool not found. Install a JDK and ensure $JAVA_HOME is set.");
        return 1;
    }
    let keytool = keytool.unwrap();

    if args.verbose {
        logger.detail(&format!("Using keytool: {}", keytool));
    }

    logger.info(&format!("{}", "Android Keystore Setup".cyan().bold()));

    // Detect project root (walk up for pubspec.yaml)
    let current_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let project_root = find_project_root(&current_dir);
    let default_keystore_path = if let Some(ref root) = project_root {
        root.join("android").join("keystore.jks")
    } else {
        PathBuf::from("android/keystore.jks")
    };

    // Interactive prompts
    let keystore_file = prompt_non_empty(
        &logger,
        &format!("Keystore file [{}]", default_keystore_path.display()),
        &default_keystore_path.to_string_lossy(),
    );

    let alias = prompt_non_empty(&logger, "Key alias [key]", "key");

    let store_password = prompt_password_confirmed(&logger, "Store password (min 6 chars)", 6);
    let key_password = prompt_password_confirmed(&logger, "Key password (min 6 chars)", 6);

    let cn = prompt_non_empty(&logger, "Your name or organization (CN)", "");
    let ou = prompt_non_empty(&logger, "Organizational unit (OU) [Mobile]", "Mobile");
    let org = prompt_non_empty(&logger, "Organization (O)", "");
    let locality = prompt_non_empty(&logger, "City or Locality (L)", "");
    let state = prompt_non_empty(&logger, "State or Province (ST)", "");
    let country = prompt_country(&logger);

    let dname = format!(
        "CN={}, OU={}, O={}, L={}, ST={}, C={}",
        cn, ou, org, locality, state, country
    );

    logger.info(&format!("\n{}", "Generating keystore...".cyan()));

    let result = runner.run(
        &keytool,
        &[
            "-genkey",
            "-v",
            "-keystore",
            &keystore_file,
            "-alias",
            &alias,
            "-keyalg",
            "RSA",
            "-keysize",
            "2048",
            "-validity",
            "10000",
            "-storepass",
            &store_password,
            "-keypass",
            &key_password,
            "-dname",
            &dname,
        ],
        None,
    );

    if !result.is_success() {
        logger.err(&format!("keytool failed: {}", result.stderr));
        if args.verbose {
            logger.err(&result.stdout);
        }
        return 1;
    }

    logger.success(&format!(
        "{} Keystore generated: {}",
        "✓".green(),
        keystore_file
    ));

    // Write android/key.properties
    let key_props_path = if let Some(ref root) = project_root {
        root.join("android").join("key.properties")
    } else {
        PathBuf::from("android/key.properties")
    };

    let keystore_abs =
        std::fs::canonicalize(&keystore_file).unwrap_or_else(|_| PathBuf::from(&keystore_file));

    let props_content = format!(
        "storePassword={}\nkeyPassword={}\nkeyAlias={}\nstoreFile={}\n",
        store_password,
        key_password,
        alias,
        keystore_abs.display()
    );

    match std::fs::write(&key_props_path, &props_content) {
        Ok(_) => logger.success(&format!(
            "{} Written: {}",
            "✓".green(),
            key_props_path.display()
        )),
        Err(e) => {
            logger.err(&format!(
                "Failed to write {}: {}",
                key_props_path.display(),
                e
            ));
            return 1;
        }
    }

    logger.info(&format!("\n{}", "Next steps:".yellow().bold()));
    logger.info("  1. Add android/key.properties to your .gitignore");
    logger.info("  2. Reference it in android/app/build.gradle.kts:");
    logger.info("     val keyProperties = Properties().apply {");
    logger.info("       load(rootProject.file(\"key.properties\").inputStream())");
    logger.info("     }");

    let app_info = AppDetector::new().detect(&current_dir);
    if app_info.project_type != ProjectType::Unknown {
        let app_id = app_info
            .android_package_id
            .as_deref()
            .or_else(|| {
                if app_info.flutter_name.is_empty() {
                    None
                } else {
                    Some(app_info.flutter_name.as_str())
                }
            })
            .unwrap_or("");
        if !app_id.is_empty() {
            logger.info(&format!("  3. App detected: {}", app_id));
        }
    }

    0
}

fn find_keytool(runner: &dyn Runner) -> Option<String> {
    if let Some(path) = runner.which("keytool") {
        return Some(path);
    }
    if let Ok(java_home) = std::env::var("JAVA_HOME") {
        let candidate = PathBuf::from(java_home).join("bin").join("keytool");
        if candidate.exists() {
            return Some(candidate.to_string_lossy().to_string());
        }
    }
    None
}

fn find_project_root(dir: &std::path::Path) -> Option<PathBuf> {
    let mut current = dir.to_path_buf();
    for _ in 0..10 {
        if current.join("pubspec.yaml").exists() {
            return Some(current);
        }
        match current.parent() {
            Some(p) if p != current => current = p.to_path_buf(),
            _ => break,
        }
    }
    None
}

fn prompt_non_empty(logger: &Logger, prompt: &str, default: &str) -> String {
    loop {
        let val = logger.prompt(prompt);
        let val = val.trim().to_string();
        if !val.is_empty() {
            return val;
        }
        if !default.is_empty() {
            return default.to_string();
        }
        logger.err("  Value cannot be empty.");
    }
}

fn prompt_password_confirmed(logger: &Logger, prompt: &str, min_len: usize) -> String {
    loop {
        let pw = logger.prompt_password(prompt);
        if pw.len() < min_len {
            logger.err(&format!(
                "  Password must be at least {} characters.",
                min_len
            ));
            continue;
        }
        let confirm = logger.prompt_password(&format!("Confirm {}", prompt));
        if pw == confirm {
            return pw;
        }
        logger.err("  Passwords do not match. Try again.");
    }
}

fn prompt_country(logger: &Logger) -> String {
    loop {
        let val = logger.prompt("Country code (2 uppercase letters, e.g. US)");
        let val = val.trim().to_string();
        if val.len() == 2 && val.chars().all(|c| c.is_ascii_uppercase()) {
            return val;
        }
        logger.err("  Country must be exactly 2 uppercase letters.");
    }
}
