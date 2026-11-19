# Verifikasi TUI Fullscreen ruvx

> Arsip verifikasi sebelum perubahan nama dari Kovr ke ruvx. Artefak JSON dan hash binary lama dipertahankan sesuai hasil pengujian aslinya.

> Baseline fullscreen sebelum penambahan reasoning effort. Cakupan dan hasil terbaru tersedia pada [effort per model](model-efforts.md#hasil-verifikasi).

Tanggal: **19 November 2026**. Binary **0.1.0**, Rust **1.89.0**, Ratatui **0.30.2**, Crossterm **0.29**. Fullscreen menggantikan inline; tidak ada dependency runtime/UI baru. Laporan [baseline core v1](validation-v1.md) tetap berlaku untuk cakupan agent/provider yang tidak berubah.

## Hasil

| Pemeriksaan | Hasil |
| --- | --- |
| Rust tests macOS arm64 | **38 unit + 10 integration, lulus** |
| Rust tests Linux aarch64 dalam container | **38 unit + 10 integration, lulus** |
| Formatting dan Clippy | `cargo fmt --check`, `cargo clippy --locked --all-targets -- -D warnings` lulus di macOS |
| Release | Build berhasil pada macOS dan Linux |
| PTY macOS dan Linux | Onboarding/key tersembunyi, paste Unicode/resize, drag auto-copy OSC52, prioritas Ctrl+C, wheel/PgDn, shell drag/deny/run, diff, cancel, resume tanpa replay, normal exit dan SIGTERM cleanup lulus |
| Tampilan | Inspeksi teks frame PTY pada 100×40, 74×28, dan 48×16; TestBackend juga mencakup terminal kecil serta draft yang tidak hilang |

Unit tests tambahan memverifikasi source offset Unicode, combining marks, indentasi/newline ketika soft wrap, seleksi selama streaming/reflow, drag melintasi viewport dengan edge scrolling, anchor scroll pada resize, pembacaan jurnal asli sesudah compaction/resume, cache berbatas, menu klik/keyboard, detail tool melalui keyboard, serta review panjang sebelum approval dan default Deny.

Clipboard helper diuji dengan executable fixture: teks diterima literal lewat stdin, termasuk karakter shell. Uji PTY memakai jalur SSH sintetis agar OSC52 dapat diperiksa tanpa mengubah clipboard OS pengguna. Test suite memeriksa payload terenkode dan envelope tmux, bukan konfirmasi clipboard dari emulator.

## Resource

| Metrik | Target | macOS arm64 | Linux aarch64 |
| --- | --- | --- | --- |
| Startup p95 | ≤500 ms | **63.835 ms** | **61.199 ms** |
| Startup median | Informasi | 62.024 ms | 56.054 ms |
| RSS idle maksimum teramati | ≤100 MB | **6.308 MB** | **5.005 MB** |
| Executable rilis | ≤40 MB | **5.945 MB** | **5.642 MB** |

MB memakai 1.000.000 byte. Host macOS: Apple M1, RAM 8 GiB, macOS 14.3 arm64. Linux: Debian Bookworm/glibc 2.36 dalam Docker LinuxKit 6.12.5 aarch64 pada host yang sama. Build/test Linux memakai satu job dan debug info dev/test dinonaktifkan; profil release tetap sama.

Startup memakai 20 sampel warm-cache dari peluncuran PTY sampai prompt siap. RSS idle diamati selama 10 detik; tanpa inference, model discovery, atau shell anak. Pengukuran tidak mengisolasi seluruh beban OS/VM host. Profil aktivitas memakai 256 file ×4 KiB, keluaran model sekitar 32 KiB, snapshot/shell, dan 256 percakapan tambahan sebelum resume. Maksimum RSS parent yang teramati:

| Tahap | macOS | Linux |
| --- | --- | --- |
| File tools + streaming | 8.602 MB | 6.767 MB |
| Snapshot + shell | 9.945 MB | 7.029 MB |
| Resume riwayat lebih besar | 7.258 MB | 4.481 MB |

Sampling `ps` dilakukan paling sering setiap 50 ms, mengecualikan child processes, dan tidak dijanjikan menangkap setiap peak sesaat. Sampel lengkap dan SHA-256 binary: [macOS](validation/fullscreen-macos-arm64.json), [Linux](validation/fullscreen-linux-arm64.json).

## Batas pengujian dan penggunaan

- Inspeksi frame memakai observer VT sederhana pada PTY dan Ratatui TestBackend; pengujian visual langsung pada beberapa emulator desktop belum dilakukan.
- Clipboard native `pbcopy`/Wayland/X11 dan penerimaan OSC52 oleh clipboard lokal belum diuji langsung. SSH/OSC52 bergantung izin emulator; status hanya menyatakan permintaan terkirim. Fallback terminal dibatasi 100 KB; seleksi maksimal 16 MiB.
- Helper clipboard tidak diinstal otomatis. Jika helper gagal atau tidak tersedia, aplikasi mencoba OSC52; penyalinan tidak memanggil LLM.
- Cache decoded/wrapping berbatas 64 blok dan sekitar 2 MiB, kecuali satu blok besar. Metadata indeks, respons live, satu artefak besar, dan salinan clipboard tetap membutuhkan memori tersendiri. Angka di atas bukan jaminan penggunaan memori untuk setiap ukuran session.
- Login/inference akun nyata, seluruh fault injection, dan platform lain tetap mengikuti gerbang terbuka pada laporan core v1. CI GitHub belum dieksekusi dalam sesi ini.

## Mengulang

```sh
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets
cargo build --release --locked
python3 scripts/verify_terminal.py --binary target/release/ruvx --benchmark --output target/validation-fullscreen.json
```

Panduan input dan clipboard tersedia pada [README](../README.md) serta [kontrak UX terminal](ruvx-v1/03-cli-and-terminal-ux.md). Config/session lama tetap dapat dibuka tanpa migrasi; `ui.viewport_rows` diterima dan diabaikan.
