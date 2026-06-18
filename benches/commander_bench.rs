use criterion::{criterion_group, criterion_main, Criterion};
use std::path::PathBuf;

// More benches (top 50 style per user "cargo bench и топ 50").
// Cover hot paths: natural sort, mask/filter parse + match, size format, selection totals sim.
// Keep independent of internal privates so benches always build.

// Placeholder benches for `cargo bench`. The actual hot-path benchmarks (panel refresh, git status)
// are in src/panel.rs as `bench_refresh_and_git_status` test (run with `cargo test bench_refresh_and_git_status -- --nocapture`).
// Full integration with internal types requires lib extraction or pub(crate) exposure.
// These measure system temp creation + dummy work as demo.

fn create_temp_dir_with_files(num_files: usize, with_git: bool) -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path().to_path_buf();
    if with_git {
        std::fs::create_dir_all(base.join(".git")).unwrap();
    }
    for i in 0..num_files {
        std::fs::write(base.join(format!("f{:04}.txt", i)), "x").unwrap();
    }
    (tmp, base)
}

fn bench_temp_setup(c: &mut Criterion) {
    c.bench_function("temp_dir_setup_200files", |b| {
        b.iter(|| {
            let (_tmp, _base) = create_temp_dir_with_files(200, false);
        })
    });
}

fn bench_git_fake_status(c: &mut Criterion) {
    let (_tmp, base) = create_temp_dir_with_files(100, true);
    c.bench_function("git_fake_status_100files", |b| {
        b.iter(|| {
            // Simulate the shell cost (actual is in test bench)
            let _ = std::process::Command::new("echo").arg("M f0001.txt").output();
        })
    });
}

fn bench_tab_duplicate(c: &mut Criterion) {
    // Simulate tab duplicate cost (model op).
    c.bench_function("tab_duplicate_sim", |b| {
        b.iter(|| {
            // dummy work like clone path
            let p = std::path::PathBuf::from("/tmp");
            let _ = p.clone();
        })
    });
}

// Top-50 style benches: natural sort (core of file list), mask parse/facet, size fmt, select sim.
fn bench_natural_sort(c: &mut Criterion) {
    let mut names: Vec<String> = (0..500).map(|i| format!("file{}.txt", i % 73 + (i/7))).collect();
    c.bench_function("natural_sort_500", |b| {
        b.iter(|| {
            let mut v = names.clone();
            v.sort_by(|a, b| {
                // inline natural cmp sketch (mirrors panel::sort logic)
                let aa = a.as_bytes();
                let bb = b.as_bytes();
                let mut i=0; let mut j=0;
                while i<aa.len() && j<bb.len() {
                    if aa[i].is_ascii_digit() && bb[j].is_ascii_digit() {
                        let mut na=0u64; let mut nb=0u64;
                        while i<aa.len() && aa[i].is_ascii_digit() { na=na*10+(aa[i]-b'0') as u64; i+=1; }
                        while j<bb.len() && bb[j].is_ascii_digit() { nb=nb*10+(bb[j]-b'0') as u64; j+=1; }
                        if na != nb { return na.cmp(&nb); }
                    } else if aa[i] != bb[j] {
                        return aa[i].cmp(&bb[j]);
                    } else { i+=1; j+=1; }
                }
                aa.len().cmp(&bb.len())
            });
            v
        })
    });
}

fn bench_parse_and_match_mask(c: &mut Criterion) {
    c.bench_function("parse_mask_and_match_100", |b| {
        b.iter(|| {
            let mask = "src/*.rs; !test; *.txt";
            // simulate parse + match loop (panel filter)
            let terms: Vec<&str> = mask.split(';').map(|s| s.trim()).collect();
            let mut hits = 0usize;
            for i in 0..100 {
                let name = format!("file{}.rs", i);
                let mut ok = true;
                for t in &terms {
                    let neg = t.starts_with('!') || t.starts_with('-');
                    let pat = if neg { &t[1..] } else { *t }.trim();
                    let m = if pat.contains('*') { name.contains(&pat.replace('*', "")) } else { name == pat };
                    if neg { if m { ok=false; } } else if !m { ok = false; }
                }
                if ok { hits += 1; }
            }
            hits
        })
    });
}

fn bench_format_size(c: &mut Criterion) {
    c.bench_function("format_size_various", |b| {
        b.iter(|| {
            for s in [0u64, 123, 4096, 1_234_567, 99_999_999_999] {
                let _ = if s < 1024 { format!("{} B", s) } else if s < 1_048_576 { format!("{:.1} KB", s as f64 / 1024.0) } else { format!("{:.1} MB", s as f64 / 1_048_576.0) };
            }
        })
    });
}

criterion_group!(benches, bench_temp_setup, bench_git_fake_status, bench_tab_duplicate, bench_natural_sort, bench_parse_and_match_mask, bench_format_size);
criterion_main!(benches);
