# Verifikasi Implementasi ruvx v1

> Arsip verifikasi sebelum perubahan nama dari Kovr ke ruvx. Artefak JSON dan hash binary lama dipertahankan sesuai hasil pengujian aslinya.

> Laporan ini merekam baseline inline sebelum perubahan fullscreen. Hasil UI dan pengukuran terbaru ada pada [verifikasi fullscreen](validation-fullscreen.md).

Implementasi v1 tersedia sebagai binary Rust interaktif. Pengujian lokal mencakup agent loop, dua protokol provider, kontrol Plan/Build, tools, session/recovery, OAuth validation, serta terminal pada macOS dan Linux. **Login dan inference dengan akun/provider nyata belum diuji.** Keberhasilan mock tidak menggantikan gerbang penerimaan live tersebut.

Tanggal verifikasi: **19 November 2026**. Versi aplikasi: **0.1.0**. Toolchain: **Rust/Cargo 1.89.0**; dependency dikunci dalam Cargo.lock. Profil rilis menggunakan optimasi ukuran, thin LTO, satu codegen unit, dan strip.

## Hasil pemeriksaan

| Pemeriksaan | Hasil dan cakupan |
| --- | --- |
| Formatting | `cargo fmt --check` lulus pada source final. |
| Clippy | `cargo clippy --locked --all-targets -- -D warnings` lulus di macOS. |
| Rust tests macOS | 25 unit tests dan 10 integration tests lulus. |
| Rust tests Linux | 25 unit tests dan 10 integration tests lulus dalam container Linux aarch64. |
| Release build | Berhasil pada macOS arm64 dan Linux aarch64. |
| PTY acceptance | Lulus pada kedua platform: wizard koneksi, key tersembunyi, Unicode/paste, resize, shell approval/denial, cancel, resume tanpa replay, serta cleanup normal/SIGTERM. |
| Pengukuran resource | Ketiga target startup, RAM idle, dan ukuran binary terpenuhi pada baseline di bawah. |
| Profil beban | File tools/streaming, snapshot/shell, dan resume riwayat lebih besar dijalankan pada kedua platform. |
| CI | Workflow macOS/Linux tersedia; belum dieksekusi oleh GitHub Actions dalam sesi ini. |

## Bukti workflow

**Repo existing:** fixture memiliki perubahan pengguna yang belum di-commit. Plan membaca file dan menolak patch yang diminta model secara tidak sah. Setelah pengguna memilih Build, agent mengedit beberapa file, satu shell command ditolak, command validasi disetujui dan exit 0, lalu diff memisahkan perubahan session dari kondisi dirty awal. Resume memulihkan Build dan history tanpa mengulang request atau tools. Skenario ini diuji melalui Responses dan Chat Completions.

**Proyek baru:** direktori kosong tanpa Git menjadi proyek kecil dengan file runnable dan validasi shell. Plan tidak membuat file. Build membuat aplikasi, menjalankan validasi yang disetujui, serta menyimpan hasil dan diff tambahan file.

Integration tests tambahan membuktikan batas putaran dan `/continue`, nilai 0 yang melewati 30 putaran, jeda saat konteks penuh tanpa trimming, compaction manual yang mempertahankan jurnal, kegagalan compaction tanpa mengganti konteks aktif, normalisasi opaque context sesudah compaction, pemuatan instruksi nested sebelum mutasi, serta stream gagal yang tidak mengeksekusi tools parsial.

Mock memberi respons deterministik agar perilaku ruvx dapat diuji. Skenario tersebut tidak mengukur kualitas penalaran model, kemampuan membangun proyek sembarang, atau keberhasilan instalasi dependensi internet.

## Bukti autentikasi

Unit/integration tests mencakup penyimpanan key 0600, bearer header pada mock provider, callback state/consent/parameter duplikat, PKCE vector, signature RSA serta klaim issuer/audience/expiry/nonce/subject, penolakan signing simetris, scope inference, dan dua refresh bersamaan yang hanya memakai refresh token lama satu kali.

Token dan JWKS dalam tests adalah fixture sintetis. Tidak ada kredensial akun pengguna, browser login nyata, atau kuota LLM yang digunakan. Profile ChatGPT yang registrasinya belum selesai mempertahankan referensi untuk retry tanpa mengaktifkan inference.

## Pengukuran resource

Host macOS: **Apple M1, RAM 8 GiB, macOS 14.3 arm64**. Linux: **container Debian Bookworm/glibc 2.36 aarch64 pada Docker LinuxKit 6.12.5** di host yang sama. Hasil Linux bukan pengukuran bare metal atau x86_64.

| Metrik | Target | macOS arm64 | Linux aarch64 |
| --- | --- | --- | --- |
| Startup prompt p95, 20 sampel | ≤500 ms | **59,756 ms** | **58,038 ms** |
| Median startup | Informasi | 56,307 ms | 55,659 ms |
| Maksimum RSS idle teramati, 10 detik | ≤100 MB | **5,767 MB** | **4,612 MB** |
| Ukuran executable rilis | ≤40 MB | **5,873 MB** | **5,577 MB** |
| RSS maksimum teramati: file tools dan streaming | Observasi | 9,290 MB | 7,279 MB |
| RSS maksimum teramati: snapshot dan shell | Observasi | 11,092 MB | 7,700 MB |
| RSS maksimum teramati: resume lebih besar | Observasi | 5,898 MB | 4,743 MB |

MB memakai 1.000.000 byte. Angka mentah, seluruh sampel startup, dan SHA-256 executable tersedia pada [hasil macOS](validation/macos-arm64.json) serta [hasil Linux](validation/linux-aarch64.json).

Startup diukur dari peluncuran melalui PTY sampai label input pertama tersedia. Harness mencakup overhead wrapper shell serta respons query posisi kursor. Kondisi warm-cache, config/session kecil, viewport 100×40, tanpa discovery/inference atau child shell aktif. Pengukuran bukan waktu login, respons model, atau cold startup pertama kali.

RSS merupakan sampel proses ruvx melalui `ps`; child shell, mock server, Python harness, dan compiler tidak dihitung. Profil beban memakai 256 file UTF-8 ×4 KiB, sekitar 32 KiB keluaran model, operasi list/search/read, snapshot sebelum/sesudah shell, serta tambahan 256 percakapan historis sintetis sebelum resume. Sampling saat aktivitas dilakukan paling sering setiap 50 ms; hasil adalah maksimum teramati, bukan jaminan menangkap peak sesaat.

Verifikasi Linux memakai satu build job dan debug info test/dev dinonaktifkan untuk membatasi beban build di VM. Pengaturan tersebut berlaku untuk proses build/test; profil aplikasi rilis tetap memakai konfigurasi yang sama dengan build macOS.

## Penyesuaian teknis berdasarkan pengujian

1. **Input terminal:** EventStream tidak dipakai. Ratatui inline clear/resize juga membaca respons posisi kursor melalui Crossterm; pembaca asinkron yang bersamaan menimbulkan timeout dalam uji PTY. Poll/read dan rendering kini berada pada task UI yang sama, sementara network/tools tetap asinkron. Perubahan ini melewati ulang acceptance kedua platform.
2. **Scope instruksi:** sebelum mutasi, dispatcher memeriksa hash instruksi target yang sudah masuk konteks. Instruksi baru/berubah dikembalikan sebagai `instructions_required` sebelum tindakan dilakukan.
3. **Compaction:** selain merangkum prefix, operasi manual menormalkan opaque metadata pada percakapan yang dipertahankan. Teks dan pasangan tools tetap ada, riwayat asli tetap di disk, dan pemilihan provider lain tidak membawa item khusus provider secara diam-diam.
4. **Recovery operasional:** `/acknowledge` mencatat peninjauan outcome uncertain tanpa menyatakan tindakan berhasil atau memutarnya ulang. `/connect <profile>` mempertahankan registrasi ChatGPT untuk reauthorization.
5. **Penyimpanan sederhana:** `/sessions` membaca header ketika diminta; startup tidak membangun indeks atau membaca isi seluruh repo. Inventory model tersimpan pada profil config, tanpa database/cache service tambahan.

## Gerbang yang masih terbuka

| Gerbang | Status |
| --- | --- |
| Akun ChatGPT yang memenuhi syarat menyelesaikan login, refresh, model discovery, dan tool cycle nyata | Belum diuji; membutuhkan akun pengguna dan consent di browser. |
| OpenAI API key dan gateway nyata menjalankan dua workflow penerimaan | Belum diuji; protokol dan kontrol telah diuji dengan mock. |
| Pemeriksaan visual manual pada beberapa emulator terminal | Belum dilakukan; bukti saat ini memakai PTY dan emulasi jawaban cursor-position. |
| Seluruh fault injection pada matriks rancangan | Belum seluruhnya dilakukan; misalnya disk penuh nyata dan panic ketika shell aktif tidak dinyatakan lulus dari tests lain. |
| Platform/arsitektur lain | Windows di luar scope; x86_64 belum diverifikasi lokal. Workflow CI menyediakan pemeriksaan macOS/Linux berikutnya. |

Implementasi dapat dijalankan dan dicoba sekarang. Status tersebut dibedakan dari pemenuhan seluruh gerbang rilis: matriks [pengujian dan penerimaan](ruvx-v1/08-testing-and-acceptance.md) tetap menjadi acuan untuk verifikasi lanjutan memakai koneksi nyata.

## Mengulang pemeriksaan

```sh
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets
cargo build --release --locked
python3 scripts/verify_terminal.py --binary target/release/ruvx --benchmark
```

`scripts/verify_terminal.py` memakai library standar Python, direktori sementara, serta mock provider loopback. Opsi `--output` menyimpan hasil JSON. Benchmark serta load profiling tersedia melalui `--benchmark`. Source tests tidak membutuhkan akun atau biaya inference.

Panduan penggunaan tersedia di [README proyek](../README.md).
