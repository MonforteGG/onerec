#[cfg(not(windows))]
fn main() {
    eprintln!("probe drives WASAPI, so it only runs on Windows");
}

#[cfg(windows)]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use std::sync::{Arc, Mutex};
    use std::thread;
    use std::time::Duration;

    use onerec::{open_loopback, open_microphone, Endpoints, Session, StagingArea};

    use probe::Meter;

    let endpoints = Endpoints::query()?;

    println!("microphones: {}", endpoints.microphones().len());
    for microphone in endpoints.microphones() {
        let mark = probe::default_mark(endpoints.default_microphone() == Some(microphone.id()));
        println!(
            "  {mark}{} [{}]",
            microphone.name(),
            microphone.id().as_str()
        );
    }

    println!("output devices: {}", endpoints.outputs().len());
    for output in endpoints.outputs() {
        let mark = probe::default_mark(endpoints.default_output() == Some(output.id()));
        println!("  {mark}{} [{}]", output.name(), output.id().as_str());
    }

    let microphone_id = endpoints
        .default_microphone()
        .ok_or("this machine has no default microphone")?
        .clone();
    let output_id = endpoints
        .default_output()
        .ok_or("this machine has no default output device")?
        .clone();

    let microphone = open_microphone(&microphone_id)?;
    let loopback = open_loopback(&output_id)?;
    println!("\nrecording 2 s from both endpoints");

    let microphone_meter = Arc::new(Mutex::new(probe::Stats::default()));
    let loopback_meter = Arc::new(Mutex::new(probe::Stats::default()));
    let staging = StagingArea::open()?.next_take()?;

    let mut session = Session::Idle;
    session.start(
        microphone_id,
        output_id,
        Meter::new(microphone, Arc::clone(&microphone_meter)),
        Meter::new(loopback, Arc::clone(&loopback_meter)),
        staging,
    );
    thread::sleep(Duration::from_secs(2));
    session.stop();

    let Session::AwaitingSave(pending) = &session else {
        return Err(format!("recording did not finish cleanly: {session:?}").into());
    };

    probe::report("microphone", &microphone_meter);
    probe::report("loopback", &loopback_meter);
    let mixed_frames = std::fs::metadata(pending.staging_file())?.len() / 8;
    println!(
        "mixed: {} frames over {:?}, degraded {}",
        mixed_frames,
        pending.elapsed(),
        pending.is_degraded()
    );
    println!(
        "staging file: {} ({} bytes)",
        pending.staging_file().display(),
        std::fs::metadata(pending.staging_file())?.len()
    );
    Ok(())
}

#[cfg(windows)]
mod probe {
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use onerec::{CaptureError, CaptureRead, CaptureSource};

    #[derive(Clone, Copy, Default)]
    pub struct Stats {
        pub frames: u64,
        pub peak: f32,
        pub lost: bool,
    }

    pub struct Meter<S> {
        inner: S,
        stats: Arc<Mutex<Stats>>,
    }

    impl<S> Meter<S> {
        pub fn new(inner: S, stats: Arc<Mutex<Stats>>) -> Self {
            Self { inner, stats }
        }
    }

    impl<S: CaptureSource> CaptureSource for Meter<S> {
        fn read(&mut self, max_wait: Duration) -> Result<CaptureRead, CaptureError> {
            let read = self.inner.read(max_wait);
            let mut stats = self.stats.lock().unwrap();
            match &read {
                Ok(CaptureRead::Frames(packet)) => {
                    stats.frames += packet.frames().len() as u64;
                    for [left, right] in packet.frames() {
                        stats.peak = stats.peak.max(left.abs()).max(right.abs());
                    }
                }
                Ok(CaptureRead::NoPacket) => {}
                Err(_) => stats.lost = true,
            }
            read
        }
    }

    pub fn report(label: &str, meter: &Arc<Mutex<Stats>>) {
        let stats = *meter.lock().unwrap();
        println!(
            "{label}: {} frames, peak {:.4}, device lost {}",
            stats.frames, stats.peak, stats.lost
        );
    }

    pub fn default_mark(is_default: bool) -> &'static str {
        if is_default {
            "* "
        } else {
            "  "
        }
    }
}
