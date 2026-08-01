//! Diagnostic: find NDI sources, and optionally receive from one.
//!
//! This is how the NDI layer gets verified without the rest of the app, and how
//! an operator answers "can this machine see Resolume at all?".
//!
//!     cargo run -p unmapper-ndi --example ndi_probe
//!     cargo run -p unmapper-ndi --example ndi_probe -- "STUDIO (Arena - Screen 1)"

use std::time::{Duration, Instant};

fn main() {
    let ndi = match unmapper_ndi::Ndi::load() {
        Ok(n) => n,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };

    println!(
        "NDI runtime {} at {}",
        if ndi.version().is_empty() {
            "(unversioned)"
        } else {
            ndi.version()
        },
        ndi.library_path().display()
    );

    let target = std::env::args().nth(1);

    print!("discovering for 3s… ");
    let sources = ndi.discover(Duration::from_secs(3)).unwrap_or_default();
    println!("{} source(s)", sources.len());
    for s in &sources {
        println!(
            "  - {}{}",
            s.name,
            match &s.url {
                Some(u) => format!("  [{u}]"),
                None => String::new(),
            }
        );
    }

    let Some(target) = target else {
        if sources.is_empty() {
            println!(
                "\nNo sources. Discovery is not instant — try again, and check that the \
                      sender is on this subnet."
            );
        } else {
            println!("\nPass a source name to receive from it.");
        }
        return;
    };

    println!("\nreceiving from {target:?} for 5s…");
    let recv = ndi.receive(&target, "UnMapper probe");
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut seen = 0u64;

    while Instant::now() < deadline {
        if let Some(frame) = recv.take_frame() {
            if seen == 0 {
                println!(
                    "  first frame: {}x{} {} stride {}",
                    frame.width,
                    frame.height,
                    frame.format.label(),
                    frame.stride
                );
            }
            seen += 1;
            recv.recycle(frame.data);
        }
        std::thread::sleep(Duration::from_millis(4));
    }

    let status = recv.status();
    println!(
        "  collected {seen} frame(s); receiver saw {}, dropped {}, {:.1} fps, connected={}",
        status.frames, status.dropped, status.fps, status.connected
    );
    if let Some(err) = status.last_error {
        println!("  last error: {err}");
    }
    if seen == 0 {
        println!(
            "\nNo frames. Check the source name matches exactly, including the machine prefix."
        );
    }
}
