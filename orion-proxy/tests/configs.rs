use orion_configuration::config::{Config, Runtime};
use orion_configuration::options::Options;
use orion_lib::RUNTIME_CONFIG;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tracing_test::traced_test;

/// we cannot run the tests concurrently because some of them rely on
/// the current working directory ($PWD). This function is just a wrapper with
/// a lock.
fn with_current_dir<F, T>(p: &Path, f: F) -> Result<T, orion_error::Error>
where
    F: FnOnce() -> T,
{
    static TEST_CURRENT_DIR_MUTEX: Mutex<()> = Mutex::new(());
    let _guard = TEST_CURRENT_DIR_MUTEX.lock().map_err(|_| orion_error::Error::from("Failed to lock test mutex"))?;
    let _ = RUNTIME_CONFIG.set(Runtime::default());
    let save =
        std::env::current_dir().map_err(|e| orion_error::Error::from(format!("Failed to get current dir: {e}")))?;
    std::env::set_current_dir(p).map_err(|e| orion_error::Error::from(format!("Failed to set current dir: {e}")))?;
    let r = f();
    std::env::set_current_dir(save)
        .map_err(|e| orion_error::Error::from(format!("Failed to restore current dir: {e}")))?;
    Ok(r)
}

fn check_config_file(file_path: &str) -> Result<(), orion_error::Error> {
    // file_path is relative to crate root
    let bootstrap = Config::new(&Options::from_path_to_envoy(file_path))?.bootstrap;
    // but anciliary files are stored in workspace root - adjust PWD
    let d = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .canonicalize()
        .map_err(|e| orion_error::Error::from(format!("Failed to get cargo crate root: {e}")))?;
    let _ = with_current_dir(&d, || orion_lib::configuration::get_listeners_and_clusters(bootstrap).map(|_| ()))?;
    Ok(())
}

#[traced_test]
#[test]
fn bootstrap_demo_static() -> Result<(), orion_error::Error> {
    check_config_file("conf/demo/demo-static.yaml")
}

#[traced_test]
#[test]
fn bootstrap_demo_dynamic() -> Result<(), orion_error::Error> {
    check_config_file("conf/demo/demo-dynamic.yaml")
}
