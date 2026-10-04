use std::{
    env, fs,
    path::PathBuf,
    process::Command,
    thread,
    time::{Duration, Instant},
};

fn main() -> std::io::Result<()> {
    let marker = PathBuf::from(env::var_os("CI_POLICY_TEST_MARKER").expect("test marker"));
    if env::args().nth(1).as_deref() == Some("worker") {
        fs::write(&marker, "started")?;
        thread::sleep(Duration::from_secs(8));
        fs::write(marker.with_extension("late"), "escaped")?;
    } else {
        let mut child = Command::new(env::current_exe()?).arg("worker").spawn()?;
        let start = Instant::now();
        while !marker.try_exists()? {
            assert!(
                start.elapsed() < Duration::from_secs(2),
                "worker did not start"
            );
            thread::sleep(Duration::from_millis(10));
        }
        if let Ok(status) = env::var("CI_POLICY_TEST_EXIT") {
            std::process::exit(status.parse().expect("test exit status"));
        }
        thread::sleep(Duration::from_secs(60));
        child.wait()?;
    }
    Ok(())
}
