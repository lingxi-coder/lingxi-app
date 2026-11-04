use colorchoice::ColorChoice;
use std::sync::{Arc, Barrier};

fn main() {
    use ColorChoice::{Always, AlwaysAnsi, Auto, Never};
    let variants = [Auto, AlwaysAnsi, Always, Never];
    assert_eq!(ColorChoice::default(), Auto);
    assert_eq!(ColorChoice::global(), Auto);
    for choice in variants {
        choice.write_global();
        assert_eq!(ColorChoice::global(), choice);
    }
    println!("all four choices round-trip; initial and default are Auto");

    let barrier = Arc::new(Barrier::new(9));
    let mut threads = Vec::new();
    for index in 0..8 {
        let barrier = barrier.clone();
        threads.push(std::thread::spawn(move || {
            let choice = variants[index % variants.len()];
            barrier.wait();
            for _ in 0..20_000 {
                if index < 4 {
                    choice.write_global();
                }
                assert!(variants.contains(&ColorChoice::global()));
            }
        }));
    }
    barrier.wait();
    for thread in threads {
        thread.join().unwrap();
    }
    Never.write_global();
    assert_eq!(ColorChoice::global(), Never);
    println!("160,000 concurrent reads and 80,000 writes completed; final write observed after join");
}
