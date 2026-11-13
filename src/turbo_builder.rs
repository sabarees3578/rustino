use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
#[derive(Clone, Debug)]
pub struct BuildPlan {
    pub build_path: PathBuf,
    pub jobs: usize,
}

/// Arduino platform recipes sometimes interpolate the sketch name into a
/// `bash -c` command without shell quoting. Keep staged names shell-safe even
/// when the user's real sketch has spaces, parentheses, or other punctuation.
pub fn cli_sketch_name(stem: &str) -> String {
    let mut safe = stem
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '_' {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    if safe.is_empty() {
        safe.push_str("Sketch");
    } else if !safe
        .chars()
        .next()
        .is_some_and(|character| character.is_ascii_alphabetic())
    {
        safe.insert_str(0, "Sketch_");
    }
    safe
}

fn cache_root() -> PathBuf {
    #[cfg(windows)]
    {
        return std::env::var_os("LOCALAPPDATA")
            .or_else(|| std::env::var_os("APPDATA"))
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir)
            .join("Rustino");
    }
    #[cfg(not(windows))]
    {
        std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
            .unwrap_or_else(std::env::temp_dir)
            .join("rustino")
    }
}

pub fn build_plan(sketch: &Path, fqbn: &str) -> BuildPlan {
    let canonical = sketch
        .canonicalize()
        .unwrap_or_else(|_| sketch.to_path_buf());
    let key = stable_key(&(canonical.to_string_lossy().into_owned(), fqbn.to_owned()));
    BuildPlan {
        build_path: cache_root().join("build").join(format!("{key:016x}")),
        jobs: std::thread::available_parallelism()
            .map(usize::from)
            .unwrap_or(1),
    }
}

/// Keep the staged sketch source path stable across app restarts so Arduino
/// CLI's incremental build cache can reuse the compile context.
pub fn staged_sketch_path(sketch: &Path) -> PathBuf {
    let canonical = sketch
        .canonicalize()
        .unwrap_or_else(|_| sketch.to_path_buf());
    let key = stable_key(&canonical.to_string_lossy().into_owned());
    let stem = sketch
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("sketch");
    cache_root()
        .join("staging")
        .join(format!("{key:016x}"))
        .join(cli_sketch_name(stem))
}
fn stable_key<T: Hash>(value: &T) -> u64 {
    struct Fnv(u64);
    impl Hasher for Fnv {
        fn finish(&self) -> u64 {
            self.0
        }
        fn write(&mut self, bytes: &[u8]) {
            for b in bytes {
                self.0 ^= u64::from(*b);
                self.0 = self.0.wrapping_mul(0x100000001b3);
            }
        }
    }
    let mut h = Fnv(0xcbf29ce484222325);
    value.hash(&mut h);
    h.finish()
}
pub fn add_compile_options(command: &mut std::process::Command, plan: &BuildPlan) {
    let _ = std::fs::create_dir_all(&plan.build_path);
    command
        .args(["--build-path"])
        .arg(&plan.build_path)
        .args(["--jobs", &plan.jobs.to_string()]);
}
