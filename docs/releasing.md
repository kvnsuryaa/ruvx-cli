# Membuat release ruvx

Workflow [release.yml](../.github/workflows/release.yml) berjalan saat tag `v*` di-push. Seluruh target harus lulus sebelum workflow membuat **draft GitHub Release**. Publikasi dilakukan setelah maintainer memeriksa draft.

## Target dan isi paket

| Perangkat | Target Rust | Runner build dan pengujian |
| --- | --- | --- |
| macOS Apple Silicon | `aarch64-apple-darwin` | `macos-15` |
| macOS Intel | `x86_64-apple-darwin` | `macos-15-intel` |
| Linux x86_64 | `x86_64-unknown-linux-gnu` | `ubuntu-22.04` |
| Linux ARM64 | `aarch64-unknown-linux-gnu` | `ubuntu-22.04-arm` |

Semua build berjalan native dengan Rust 1.89.0 dan `Cargo.lock` yang sama. Label runner mengikuti [dokumentasi GitHub](https://docs.github.com/en/actions/reference/runners/github-hosted-runners). Paket Linux memakai glibc dari Ubuntu 22.04 (2.35); kompatibilitas distro lebih lama dan musl belum diverifikasi. Pengujian paket macOS memakai macOS 15; kompatibilitas versi lain belum diverifikasi. Developer ID signing/notarization macOS belum tersedia dalam pipeline ini.

Setiap `ruvx-v<VERSION>-<TARGET>.tar.gz` berisi satu direktori dengan binary executable `ruvx`, `README.md`, `LICENSE`, dan `docs/` agar tautan dokumentasi lokal tetap tersedia. Config, kredensial, session, build cache, dan source aplikasi tidak ikut dikemas. File `SHA256SUMS` pada release mencakup keempat arsip.

## Persiapan pertama

- Pastikan GitHub Actions aktif dan action resmi `actions/checkout`, `actions/setup-python`, `actions/upload-artifact`, serta `actions/download-artifact` diizinkan.
- Workflow menggunakan `GITHUB_TOKEN` bawaan dengan `contents: write` hanya pada job pembuat draft. Tidak perlu PAT atau secret tambahan. Kebijakan organisasi/repository perlu mengizinkan pembuatan release oleh Actions.
- Commit dan push seluruh file workflow, skrip, lisensi, dan dokumentasi sebelum membuat tag. Tag harus menunjuk commit yang sudah memuat `release.yml`.

## Menyiapkan versi

Versi awal di manifest adalah `0.1.0`. Untuk versi berikutnya, ubah `[package].version` di `Cargo.toml`, lalu perbarui entri paket lokal dalam lockfile:

```sh
cargo check
git diff -- Cargo.toml Cargo.lock
```

Review perubahan lockfile, lalu jalankan pemeriksaan lokal berikut. Skrip release memerlukan Python 3.11 atau lebih baru; CI menggunakan Python 3.12.

```sh
python3 scripts/release.py check-version v0.1.0
python3 -m unittest discover -s scripts -p 'test_release.py'
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets
cargo build --release --locked
python3 scripts/verify_terminal.py --binary target/release/ruvx --benchmark
```

Ganti `v0.1.0` sesuai versi manifest. Tag harus tepat `v` diikuti versi di `Cargo.toml`, dan versi paket lokal di `Cargo.lock` harus sama. Untuk prerelease, gunakan versi seperti `0.2.0-rc.1` pada manifest/lockfile dan tag `v0.2.0-rc.1`; draft otomatis ditandai sebagai prerelease.

Commit perubahan yang sudah diperiksa, push commit, lalu push tag anotasi:

```sh
git push origin main
git tag -a v0.1.0 -m "Release ruvx v0.1.0"
git push origin v0.1.0
```

Gunakan branch rilis yang sesuai bila bukan `main`. Sebelum tagging, pastikan working tree bersih dan commit yang ditag merupakan versi yang ingin dirilis.

## Yang dijalankan otomatis

1. Memastikan tag, manifest, dan lockfile cocok; menguji skrip release; menjalankan format dan Clippy.
2. Menjalankan Rust test suite, build release, dan PTY acceptance/benchmark pada keempat target. Versi binary juga diperiksa sebelum pengemasan.
3. Mengunggah paket serta laporan `validation-<TARGET>` sebagai artifact workflow.
4. Mengumpulkan paket, menolak paket yang kurang/berlebih, lalu menghasilkan dan memeriksa `SHA256SUMS`.
5. Membuat draft release dengan keempat paket, checksum, dan catatan perubahan otomatis dari GitHub.

Jika satu target gagal, draft baru tidak dibuat. Buka tab **Actions → release**, periksa log, lalu rerun job yang gagal untuk masalah sementara. Workflow dapat memperbarui asset draft pada rerun; release yang sudah dipublikasikan tidak akan ditimpa. Jika kode perlu diperbaiki, commit perbaikan dan gunakan versi/tag baru agar tag yang sudah di-push tidak dipindahkan.

## Memeriksa dan memublikasikan

Buka **Releases** pada repository dengan akun maintainer. Draft hanya terlihat oleh akun dengan akses yang sesuai; pengguna umum belum dapat mengunduhnya.

- Pastikan tersedia empat `.tar.gz` dan satu `SHA256SUMS` untuk versi yang sama.
- Unduh paket yang sesuai perangkat, verifikasi checksum, ekstrak, lalu coba `./ruvx --version`, `./ruvx --help`, dan jalankan pada workspace sementara. Lihat instruksi instalasi di README.
- Periksa catatan perubahan otomatis dan tambahkan perubahan penting, batas kompatibilitas, serta masalah yang diketahui. Test mock/PTY tidak membuktikan login dan inference provider nyata; status pengujian live dicatat di [laporan verifikasi](validation-v1.md).
- Setelah pemeriksaan selesai, klik **Publish release**. Prerelease tetap ditandai sebagai prerelease.

Perilaku `--draft`, `--verify-tag`, dan catatan perubahan otomatis mengikuti [dokumentasi GitHub CLI](https://cli.github.com/manual/gh_release_create).

## Mengemas binary secara lokal

Setelah build dan pengujian pada perangkat yang sesuai, contoh untuk macOS Apple Silicon:

```sh
python3 scripts/release.py package v0.1.0 \
  --target aarch64-apple-darwin --binary target/release/ruvx
```

Hasil berada di `target/dist/`. Argumen `--target` memberi nama paket; build dan pengujian tetap harus dilakukan pada OS/arsitektur yang benar. Workflow memverifikasi host Rust sebelum build.

Setelah keempat paket dari versi yang sama terkumpul:

```sh
python3 scripts/release.py checksums v0.1.0
```

Pemeriksaan kelengkapan sengaja menolak direktori yang hanya berisi satu paket atau memiliki arsip dari versi lama. Untuk memeriksa satu paket unduhan, gunakan perintah checksum dengan `--ignore-missing` dari README dan pastikan nama paket tersebut mendapat status `OK`.
