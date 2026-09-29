//! Mide la velocidad de AirDrop por loopback (sin red de por medio: lo que se
//! mide es solo el software y el disco). Uso: bench_airdrop <MB> [archivos]
use std::io::Write;
use std::time::{Duration, Instant};

use liquidcontrol_core::airdrop::{self, Peer};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mb: u64 = args.get(1).and_then(|a| a.parse().ok()).unwrap_or(2048);
    let count: usize = args.get(2).and_then(|a| a.parse().ok()).unwrap_or(1);
    let base = std::env::temp_dir().join("liquidcontrol-bench");
    let (src, dst) = (base.join("origen"), base.join("destino"));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&src).unwrap();

    let chunk: Vec<u8> = (0..1024 * 1024).map(|i: u32| (i.wrapping_mul(2_654_435_761) >> 24) as u8).collect();
    let mut paths = Vec::new();
    for n in 0..count {
        let path = src.join(format!("grande-{n}.bin"));
        let mut file = std::io::BufWriter::new(std::fs::File::create(&path).unwrap());
        for _ in 0..mb / count as u64 {
            file.write_all(&chunk).unwrap();
        }
        paths.push(path);
    }
    let total: u64 = paths.iter().map(|p| std::fs::metadata(p).unwrap().len()).sum();

    airdrop::start("Bench", dst.clone()).unwrap();
    std::thread::sleep(Duration::from_millis(300));
    std::thread::spawn(|| {
        loop {
            std::thread::sleep(Duration::from_millis(20));
            if airdrop::status().pending.is_some() {
                airdrop::decide(true);
            }
        }
    });
    let peer = Peer {
        fingerprint: "yo".into(),
        alias: "yo".into(),
        model: String::new(),
        kind: "desktop".into(),
        ip: "127.0.0.1".into(),
        port: 53317,
    };
    let started = Instant::now();
    airdrop::send(&peer, &paths).unwrap();
    let secs = started.elapsed().as_secs_f64();
    let received: u64 = std::fs::read_dir(&dst).unwrap().flatten().map(|e| e.metadata().unwrap().len()).sum();
    println!(
        "{count} archivo(s), {:.0} MB en {secs:.2} s → {:.0} MB/s (recibido completo: {})",
        total as f64 / 1e6,
        total as f64 / 1e6 / secs,
        received == total
    );
    airdrop::stop();
    let _ = std::fs::remove_dir_all(&base);
}
