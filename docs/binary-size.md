# Ukuran binary ruvx

Pengukuran ukuran instalasi memakai executable release yang sudah di-strip.
Direktori `target/` mencakup dependency dan cache build, sehingga ukurannya bukan
ukuran aplikasi yang perlu dipasang. Seluruh varian di bawah mempertahankan fitur,
dependency, dan strategi panic yang sama.

## Hasil perbandingan compiler

Pengukuran lokal pada 27 November 2026 memakai macOS 26.0 arm64, RAM 8 GiB,
Rust/Cargo 1.89.0, dan lockfile yang sama untuk semua varian. Baseline dibangun
ulang dari source saat eksperimen, termasuk perubahan lokal yang sudah ada;
binary lama 6.656.048 byte tidak dipakai untuk menghitung penghematan.

| `opt-level` | `lto` | Executable, byte | Penghematan dari baseline | Durasi build, detik |
| --- | --- | ---: | ---: | ---: |
| `"s"` | `"thin"` (baseline) | 6.697.472 | 0% | 110,961 |
| `"z"` | `"thin"` | 5.657.424 | 15,53% | 87,740 |
| `"s"` | `"fat"` | 5.263.760 | 21,41% | 147,379 |
| `"z"` | `"fat"` | 4.252.400 | 36,51% | 195,384 |

Setiap direktori build dimulai tanpa cache artefak Cargo; registry dependency
sudah tersedia. Durasi diukur berurutan dengan clock monotonic dan mencakup
peluncuran Cargo. Cache OS, suhu, dan beban host tidak dikontrol, sehingga angka
durasi merupakan observasi, bukan benchmark kompilasi berulang. Hasil ukuran ini
hanya mengukur macOS Apple Silicon; platform lain perlu pengukuran sendiri.

Konfigurasi `"z"` + `"fat"` menghasilkan executable terkecil: **4,2524 MB**
(4,055 MiB), menghemat **2.445.072 byte / 36,51%** dari baseline. Konfigurasi ini
dipilih dalam `Cargo.toml`; `codegen-units = 1` dan `strip = true` tetap digunakan.
Penghematan disertai durasi build teramati yang lebih lama.

Validasi awal varian `z-fat` sempat gagal saat harness membaca konfigurasi MCP
segera setelah perintah Save; pengulangan pada binary identik lolos. Harness
sebelumnya hanya memompa PTY selama 120 ms sebelum membaca hasil. Pemeriksaan
sekarang menunggu entri server benar-benar tersimpan sebelum menguji isinya,
dengan timeout yang sama seperti penantian kondisi lain. Assertion konfigurasi
tetap dijalankan; batas resource tidak dilonggarkan.

Keempat varian lolos acceptance terminal, alur Git lokal/autentikasi mock,
kebijakan approval/BYPASS, profil beban, dan paste 1/10 MiB. Benchmark memakai
Python 3.11.14, 20 startup warm-cache dan observasi idle 10 detik per varian:

| Varian | Startup median, ms | Startup p95, ms | Maksimum RSS idle, byte |
| --- | ---: | ---: | ---: |
| `s-thin` | 60,345 | 120,158 | 9.076.736 |
| `z-thin` | 56,972 | 63,527 | 9.158.656 |
| `s-fat` | 55,819 | 174,984 | 8.978.432 |
| `z-fat` | 55,858 | 73,600 | 9.011.200 |

Semua memenuhi ambang startup p95 ≤500 ms dan RSS idle ≤100 MB. Variasi startup
dan RSS pada host bersama ini tidak membuktikan satu varian selalu lebih cepat
atau hemat RAM. Pemilihan mengutamakan ukuran executable yang memenuhi validasi.

Sampel mentah, hash binary/source/lockfile, ringkasan acceptance, serta atribusi
kode tersimpan dalam [laporan terstruktur](validation/binary-size-macos-arm64.json).
Log build dan laporan lengkap tiap varian tersedia secara lokal di
`target/size-study/`.

Test suite dengan profil release terpilih juga lulus: **193 test** (160 unit test
library, 28 alur agent, dan 5 koneksi Claude; executable utama memiliki 0 unit
test). Perintah memakai Rust 1.89.0, `--release --locked --all-targets -j 1`.
Binary utama hasil `cargo build` di `target/release/ruvx` berukuran
**4.252.400 byte**; checksum-nya tercatat terpisah dari binary eksperimen dalam
laporan JSON. Binary utama juga lolos seluruh acceptance dan benchmark terminal:
startup median 54,627 ms, p95 56,121 ms, serta maksimum RSS idle 9.011.200 byte.

## Mengulang perbandingan compiler

Jalankan dari root repository. Gunakan Rust 1.89.0 seperti CI dan Python 3.11+
(contoh ini memakai executable `python3.11`). Pastikan tidak ada override lain
seperti `RUSTFLAGS` atau `CARGO_PROFILE_RELEASE_*` yang memengaruhi perbandingan.
Keempat direktori build terpisah; build pertama di setiap direktori mengompilasi
dependency dari awal. Durasi build mencakup pengaruh cache OS dan beban host.

```sh
mkdir -p target/size-study
for variant in s-thin z-thin s-fat z-fat; do
  size_opt="${variant%-*}"
  size_lto="${variant#*-}"
  size_dir="target/size-study/$variant"
  mkdir -p "$size_dir"
  /usr/bin/time -p env \
    CARGO_TARGET_DIR="$size_dir" \
    CARGO_PROFILE_RELEASE_OPT_LEVEL="$size_opt" \
    CARGO_PROFILE_RELEASE_LTO="$size_lto" \
    CARGO_PROFILE_RELEASE_CODEGEN_UNITS=1 \
    CARGO_PROFILE_RELEASE_STRIP=true \
    cargo +1.89.0 build --release --locked --bin ruvx \
    > "$size_dir/build.log" 2>&1 || break
done
```

Setelah seluruh build selesai, jalankan validasi satu per satu tanpa compiler
aktif agar benchmark tidak bersaing dengan proses build:

```sh
for variant in s-thin z-thin s-fat z-fat; do
  size_dir="target/size-study/$variant"
  "$size_dir/release/ruvx" --help > /dev/null || break
  "$size_dir/release/ruvx" --version || break
  python3.11 scripts/verify_terminal.py \
    --binary "$size_dir/release/ruvx" --benchmark \
    --output "$size_dir/validation.json" \
    > "$size_dir/validation.log" 2>&1 || break
done
```

Laporan JSON memuat `performance.binary_bytes`, startup median/p95 dari 20 sampel,
dan maksimum RSS idle selama 10 detik. Ambang harness: startup p95 ≤500 ms, RSS
idle ≤100.000.000 byte, executable ≤40.000.000 byte. Startup memakai warm-cache
dan PTY; RSS hanya proses ruvx, tidak termasuk child process.

Pilih executable terkecil yang lolos validasi. Jika ukuran sama, pertahankan
konfigurasi sebelumnya bila termasuk kandidat; selebihnya utamakan `thin`, lalu
`"s"`. Ubah hanya `opt-level` dan `lto` dalam `[profile.release]`, kemudian jalankan
test suite dan build normal:

```sh
cargo +1.89.0 test --release --locked --all-targets -j 1
cargo +1.89.0 build --release --locked --bin ruvx
python3.11 scripts/verify_terminal.py --binary target/release/ruvx --benchmark \
  --output target/size-study/final-validation.json
```

## Menganalisis kontribusi kode

`cargo-bloat` merupakan alat pengembangan terpisah dan tidak menjadi dependency
aplikasi. Gunakan profil yang sudah dipilih dalam `Cargo.toml`:

```sh
cargo +1.89.0 install cargo-bloat --version 0.12.1 --locked
mkdir -p target/size-study/analysis
CARGO_TARGET_DIR=target/size-study/analysis \
  cargo +1.89.0 bloat --release --locked --bin ruvx --crates -n 0 \
  --message-format json > target/size-study/analysis/crates.json
CARGO_TARGET_DIR=target/size-study/analysis \
  cargo +1.89.0 bloat --release --locked --bin ruvx -n 20 \
  --message-format json > target/size-study/analysis/functions.json
```

`cargo-bloat` 0.12.1 menonaktifkan stripping agar simbol dapat dianalisis. Direktori
analisis sengaja terpisah supaya executable instalasi tetap di-strip. Kontribusi
crate/fungsi adalah perkiraan berdasarkan simbol pada bagian kode (`.text`),
bukan seluruh ukuran executable; data konstan, tabel Unicode, metadata, alignment,
dan informasi simbol tidak seluruhnya tercakup. LTO dan inlining juga dapat
memengaruhi atribusi. Ukuran file build analisis tidak dipakai sebagai ukuran
instalasi, dan angka per crate bukan jaminan penghematan bila dependency dihapus.

### Hasil `cargo-bloat` pada konfigurasi terpilih

Build analisis berukuran 7.579.416 byte karena menyertakan simbol, dengan bagian
kode terlapor 2.747.552 byte. Ukuran instalasi tetap **4.252.400 byte**, diambil
dari build yang di-strip. Berikut kontribusi kode terbesar; persentase memakai
bagian kode build analisis sebagai pembagi, bukan ukuran file instalasi:

| Crate | Perkiraan kode, byte | Bagian kode |
| --- | ---: | ---: |
| `ruvx` | 680.676 | 24,77% |
| `std` | 486.200 | 17,70% |
| `rustls` | 205.220 | 7,47% |
| `[Unknown]` | 181.424 | 6,60% |
| `regex_automata` | 141.816 | 5,16% |
| `reqwest` | 116.644 | 4,25% |
| `ring` | 94.516 | 3,44% |
| `clap_builder` | 92.532 | 3,37% |
| `regex_syntax` | 79.652 | 2,90% |
| `aho_corasick` | 68.156 | 2,48% |
| `tokio` | 60.692 | 2,21% |
| `toml_edit` | 54.648 | 1,99% |

`[Unknown]` berarti alat tidak dapat mengatribusikan simbol ke crate tertentu.
Kontribusi `ruvx` juga dapat mencakup kode generic/inlining dari dependency.
Untuk audit lanjutan, TLS/kriptografi (`rustls`, `ring`) dan pemrosesan pola
(`regex_automata`, `regex_syntax`, `aho_corasick`) layak diperiksa bersama fungsi
yang memakainya. Pengukuran ini belum membuktikan fitur dependency dapat dimatikan
tanpa mengubah perilaku aplikasi.

| Fungsi terbesar | Perkiraan kode, byte |
| --- | ---: |
| `ruvx::start::{{closure}}` | 33.768 |
| `ruvx::agent::run::{{closure}}` | 27.156 |
| `ruvx::ui::screen::render` | 20.188 |
| `ruvx::mcp::Manager::execute::{{closure}}` | 19.776 |
| `regex_automata::meta::strategy::new` | 16.752 |

Daftar lengkap crate dan 20 fungsi terbesar tersedia pada
`target/size-study/analysis/crates.json` dan `functions.json` setelah menjalankan
perintah di atas.
