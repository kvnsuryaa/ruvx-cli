# Reasoning effort per model

ruvx menyimpan pilihan reasoning effort secara terpisah untuk setiap **profil + model**. Berpindah model memulihkan pilihan model tersebut. Perubahan hanya berlaku pada request berikutnya, tanpa mengubah Plan/Build, memulai tugas, atau memanggil model. Effort tidak dapat diubah selama pekerjaan aktif.

## Menggunakan TUI

Jalankan `/effort` atau klik label **[Effort: …]** pada baris status. Pilih dengan mouse, arrows/Tab + Enter, nama level, atau nomor pilihan. Escape membatalkan dan memulihkan draft.

```text
/effort
/effort high
/effort default
```

**Provider default** berarti parameter effort tidak dikirim. Nilai **none** adalah pilihan berbeda: ruvx secara eksplisit meminta tidak memakai reasoning, dan hanya mengizinkannya jika termasuk level yang didukung. Label `default` tidak mengklaim bahwa default remote pasti `medium`, `low`, atau `none`.

Pilihan bernama `none`, `minimal`, `low`, `medium`, `high`, `xhigh`, dan `max` tersedia hanya sesuai kapabilitas model. Effort lebih tinggi umumnya menambah waktu respons dan penggunaan reasoning tokens. Tidak ada proses LLM lokal tambahan untuk fitur ini. Format dan sifat model-dependent mengikuti [panduan reasoning OpenAI](https://developers.openai.com/api/docs/guides/reasoning).

## Dukungan model

Urutan resolusi daftar level:

1. `reasoning_efforts` yang tersimpan pada konfigurasi model, termasuk daftar yang diisi dari inventory.
2. Preset ID model OpenAI/ChatGPT yang dikenal, dengan mencocokkan ID wire `model_id` jika model memakai alias lokal.
3. Dukungan belum diketahui: hanya provider default dan pilihan mengatur daftar level yang ditampilkan.

Discovery membaca `supported_reasoning_levels[].effort` bila inventory menyediakannya dan model belum memiliki daftar tersimpan. Nilai tidak dikenal diabaikan, duplikasi dibuang, dan discovery tidak mengubah pilihan effort pengguna atau menyalin default remote menjadi pilihan eksplisit. Kontrak metadata mengikuti [tipe model resmi Codex](https://github.com/openai/codex/blob/main/codex-rs/protocol/src/openai_models.rs) dan [inventory bawaan resminya](https://github.com/openai/codex/blob/main/codex-rs/models-manager/models.json). Daftar tersimpan memiliki prioritas; ubah melalui `/effort levels` atau hapus field konfigurasi untuk melakukan discovery ulang.

Preset konservatif diverifikasi pada **19 November 2026**:

| ID model OpenAI/ChatGPT | Pilihan preset |
| --- | --- |
| `gpt-6.2-astra`, `gpt-6.2-sol`, `gpt-6-astra`, `gpt-6-sol`, `gpt-6-luna`, `gpt-6.1-sol`, `gpt-6.1-terra` | low, medium, high, xhigh, max |
| `gpt-5.6-sol`, `gpt-5.6-terra`, `gpt-5.6-luna` | low, medium, high, xhigh, max |
| `gpt-5.5` | none, low, medium, high, xhigh |
| `gpt-5.2` | none, low, medium, high, xhigh |
| `gpt-5.1` | none, low, medium, high |
| `gpt-5` | minimal, low, medium, high |

Preset menerima ID persis dan snapshot dengan suffix tanggal ISO yang valid, misalnya `gpt-5.2-2025-12-11`. Suffix sembarang dan model baru tidak ditebak melalui prefix. Sumber khusus: [GPT-5.2](https://developers.openai.com/api/docs/models/gpt-5.2), [GPT-5.1](https://developers.openai.com/api/docs/models/gpt-5.1), [GPT-5](https://developers.openai.com/api/docs/models/gpt-5), dan [panduan reasoning untuk GPT-5.5](https://developers.openai.com/api/docs/guides/reasoning). Preset adalah daftar lokal yang dapat diperbarui/ditimpa, bukan bukti akses akun atau daftar exhaustive semua kapabilitas API masa depan.

`ultra` tidak dipetakan dari label client Codex menjadi parameter effort API. Fitur delegasi otomatis pada client lain berada di luar cakupan ruvx.

## Model custom dan gateway

ruvx tidak menerapkan preset OpenAI secara otomatis pada profil `compatible`, termasuk model yang namanya mirip OpenAI. Tetapkan daftar yang benar-benar diterima gateway tersebut:

```text
/effort levels low medium high
/effort high
```

Tanpa argumen daftar, `/effort levels` membuka editor kapabilitas di TUI. Gunakan `unsupported` untuk daftar kosong (model tanpa kontrol effort). Untuk meminta no reasoning secara eksplisit, sertakan `none` sebagai level, misalnya `/effort levels none low high`.

Konfigurasi contoh:

```toml
[providers.gateway.models."my-model"]
supports_tools = true
supports_streaming = true
reasoning_efforts = ["low", "medium", "high"]
reasoning_effort = "high"
```

Hapus `reasoning_effort` atau gunakan `/effort default` untuk menyerahkan pilihan ke provider. Daftar kosong `reasoning_efforts = []` menyatakan tanpa kontrol effort; field yang tidak ada berarti belum ditetapkan dan dapat memakai metadata/preset yang sesuai. Level aktif harus masuk daftar; jika mengurangi daftar, reset level aktif dahulu bila perlu. Daftar duplikat dan nama level tidak dikenal ditolak.

Menetapkan daftar tidak melakukan probe dan tidak membuktikan dukungan remote. Profil compatible harus menerima parameter `reasoning_effort` dari Chat Completions; vendor dengan format thinking/budget berbeda membutuhkan adapter tersendiri. Provider yang menolak request tetap menampilkan error aslinya; ruvx tidak menurunkan level atau mengulang tindakan secara otomatis.

## Request, penyimpanan, dan verifikasi

- Responses (OpenAI API dan akun ChatGPT): `reasoning: { "effort": "high" }`.
- Chat Completions compatible: `reasoning_effort: "high"`.
- Default provider: kedua field tidak dikirim.
- Setiap putaran tool dan compaction manual memakai pilihan model yang sama pada konfigurasi run. Perubahan ditahan sampai run idle.
- Config lama tanpa field baru tetap valid; tidak ada migrasi session. Jurnal mencatat perubahan pengaturan dan effort request pada `run.started`/`context.compacted` untuk audit. Resume membaca pilihan model dari session serta preferensi per model dari config saat ini, tanpa replay request atau mengembalikan config global ke nilai historis.

Tes mencakup kedua bentuk payload, default vs none, penolakan level tidak didukung, metadata inventory, alias/snapshot model, per-model persistence/resume, menu klik/keyboard, draft, larangan mengubah saat aktif, seluruh putaran tool, dan compaction. PTY juga memeriksa pemilihan lewat mouse, konfigurasi tersimpan, parameter pada mock HTTP, dan resume. Tidak ada akun/provider nyata yang dipakai dalam tes.

## Hasil verifikasi

> Angka berikut merekam baseline fitur effort sebelum perubahan percakapan/paste. Hasil terbaru tersedia pada [panduan percakapan](conversation-and-paste.md#hasil-pengukuran).

Build final pada **19 November 2026**:

- **46 unit tests + 11 integration tests (57 total)** lulus di macOS arm64 dan Linux aarch64.
- `cargo fmt --check`, Clippy seluruh target dengan `-D warnings`, dan build release lulus di macOS; build release Linux juga lulus.
- PTY kedua platform lulus, termasuk pemilihan effort lewat mouse, persistensi config, nilai pada mock request HTTP, resume tanpa replay, serta regresi paste/seleksi/approval/cancel/cleanup.
- Inspeksi frame PTY pada 100×40 dan 74×28 memastikan seluruh pilihan sampai `max` dan editor kapabilitas terlihat serta dapat diklik pada ukuran tersebut. Tidak ada dependency baru.

| Metrik | macOS arm64 | Linux aarch64 | Target |
| --- | --- | --- | --- |
| Startup p95 | 60.304 ms | 59.819 ms | ≤500 ms |
| RSS idle maksimum teramati | 6.586 MB | 5.267 MB | ≤100 MB |
| Executable release | 6.015 MB | 5.708 MB | ≤40 MB |

MB memakai 1.000.000 byte. Startup: 20 sampel warm-cache melalui PTY 100×40 dengan config/session kecil; RAM idle: 10 detik, tanpa inference/discovery/shell anak. Host: Apple M1 8 GiB, macOS 14.3; Linux: Debian Bookworm/glibc 2.36 aarch64 di Docker LinuxKit 6.12.5 pada host yang sama. Bukan hasil bare metal Linux/x86_64 atau jaminan peak untuk semua ukuran session.

Sampel lengkap, profil beban file/streaming/snapshot/resume, serta hash executable tersedia pada [hasil macOS](validation/efforts-macos-arm64.json) dan [hasil Linux](validation/efforts-linux-arm64.json). Metode dan batas pengukuran mengikuti [laporan fullscreen](validation-fullscreen.md). Login/inference akun nyata dan clipboard emulator desktop tetap belum diverifikasi secara langsung.

```sh
cargo test --locked --all-targets
cargo clippy --locked --all-targets -- -D warnings
cargo build --release --locked
python3 scripts/verify_terminal.py --binary target/release/ruvx --benchmark --output target/validation-efforts.json
```
