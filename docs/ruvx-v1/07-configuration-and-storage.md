# Konfigurasi dan Penyimpanan ruvx v1

Konfigurasi ruvx bersifat lokal per pengguna. V1 memakai TOML untuk pengaturan dan profil, JSONL untuk jurnal session, serta file artefak terpisah untuk keluaran panjang dan baseline diff. Tidak ada database atau service storage wajib.

## Lokasi data

Default teknis mengikuti lokasi XDG pada macOS dan Linux agar perilakunya sama di kedua platform. Pemilihan ini merupakan rancangan ruvx, bukan klaim mengenai lokasi native semua aplikasi macOS.

| Data | Lokasi default |
| --- | --- |
| Config utama | `$XDG_CONFIG_HOME/ruvx/config.toml`, fallback `~/.config/ruvx/config.toml` |
| Credentials dan host identity | Direktori config yang sama; berkas privat terpisah dari config utama |
| Session dan artefak | `$XDG_DATA_HOME/ruvx/sessions/<session-id>/`, fallback `~/.local/share/ruvx/sessions/<session-id>/` |
| Model terakhir | `$XDG_DATA_HOME/ruvx/last-model.json`, fallback `~/.local/share/ruvx/last-model.json`; pasangan `profile` dan `model` |
| Indeks workspace | Opsional; implementasi v1 membaca header session ketika `/sessions` dijalankan, tanpa membangun indeks saat startup |
| Cache inventory model | Inventory terakhir disimpan pada profil config. Direktori `$XDG_CACHE_HOME/ruvx/` dilindungi sebagai lokasi cache aplikasi, tetapi belum memerlukan berkas inventory terpisah |
| Lock | Berdekatan dengan data yang dilindungi; gunakan advisory lock OS |

Nilai XDG yang dipakai harus berupa absolute path. Session tidak ditulis ke repo proyek. Directory aplikasi privat memakai permission 0700 dan file data privat 0600 pada Unix. Penulisan pengaturan dan kredensial menggunakan temporary file pada filesystem yang sama lalu atomic replace.

Penyimpanan terpisah: pada setiap root XDG, ruvx selalu memakai direktori `ruvx`, termasuk ketika hanya direktori `kovr` yang sudah ada. Config, kredensial, pilihan model, jurnal, dan lock Kovr tidak dibaca atau diubah. Tidak ada fallback, pemindahan, penyalinan, atau penggabungan data otomatis. Hubungkan provider kembali melalui `/connect` untuk memulai dengan config ruvx baru. Alias privat kedua nama tetap dilindungi dari tools file agar kredensial lama tidak terekspos. Session tetap terikat workspace kanonis yang dicatat; rename folder proyek tidak menulis ulang path pada jurnal lama.

Untuk pengujian terisolasi, variabel XDG dapat diarahkan ke direktori sementara. ruvx tidak memuat `.env` proyek secara otomatis dan tidak membaca pengaturan eksekusi dari repo.

Verifikasi pemisahan penyimpanan: **91 unit + 12 integration tests (103 total)**, format `src/config.rs`, Clippy seluruh target, release build, dan instalasi command global lulus pada macOS arm64. Uji terminal dengan root XDG sementara yang hanya berisi direktori `kovr` memastikan config legacy yang tidak valid diabaikan, session baru ditulis ke `ruvx`, data legacy tetap utuh, permission privat benar, dan terminal dipulihkan tanpa request provider. Pengujian perubahan ini belum dijalankan di Linux. Bukti: [macOS](../validation/isolated-storage-macos-arm64.json).

Arsip verifikasi perubahan nama sebelum pemisahan penyimpanan: **91 unit + 12 integration tests (103 total)**, release build, dan acceptance PTY lulus pada macOS arm64 dan Linux aarch64; format dan Clippy lulus di macOS. PTY saat itu memeriksa branding baru serta fallback default/model/kredensial pada direktori legacy; perilaku fallback tersebut sudah diganti dengan penyimpanan terpisah di atas. Tes provider memastikan namespace baru dan riwayat opaque legacy tetap konsisten. Folder sumber sudah menjadi `ruvx-cli`; command global macOS `ruvx` dipasang dan hash-nya cocok dengan release yang diuji saat itu. Instalasi global lama yang berasal dari proyek ini dilepas. Bukti historis: [macOS](../validation/rename-ruvx-macos-arm64.json), [Linux](../validation/rename-ruvx-linux-arm64.json).

## Urutan konfigurasi

1. Default aplikasi.
2. Config dan preferensi global pengguna.
3. Pilihan interaktif session, misalnya mode dan profil/model aktif.

Argumen CLI memilih workspace atau session yang dibuka, bukan mekanisme override tersembunyi untuk semua pengaturan. V1 tidak menambahkan config proyek. Dengan demikian, clone repo tidak mengubah batas putaran, executor shell, atau aturan persetujuan.

Konfigurasi divalidasi sebelum menjalankan agent. Nilai tak dikenal ditolak dengan nama field agar salah ketik tidak diam-diam diabaikan. Kesalahan tidak mencetak seluruh config atau kredensial. Perubahan file config dibaca saat startup serta `/new` atau `/resume`; tidak ada pemantauan perubahan atau penggantian model otomatis pada session yang sedang berjalan.

## Model terakhir pengguna

Setiap pemilihan model yang berhasil melalui `/model`, menu TUI, atau onboarding menyimpan pasangan profil/model sebagai default global. File ini terpisah dari `config.toml`, sehingga mengganti model tidak menulis ulang konfigurasi provider. Effort tetap memakai pengaturan per model pada config. Session yang sudah memiliki model mempertahankannya; session tanpa model memakai default global yang masih valid. Pemulihan dicatat ke jurnal session dan tidak mengubah default global maupun memulai request.

Penulisan memakai atomic replace dengan permission 0600 dan advisory lock `last-model.lock`. Pilihan terakhir yang berhasil disimpan menang; window lain membaca nilai terbaru ketika startup atau berpindah session. Disconnect hanya menghapus preferensi bila nilai terbaru menunjuk profil yang dilepas. File belum ada berarti belum ada default; tidak dilakukan migrasi dari tebakan session terbaru. Setelah pembaruan, satu pemilihan eksplisit mulai menyimpan default.

Preferensi rusak, tidak terbaca, atau menunjuk model yang hilang/tidak mendukung tools dan streaming menghasilkan pemberitahuan dan petunjuk `/model` atau `/connect`. Tidak ada fallback ke model lain. Kegagalan menyimpan default tidak membatalkan pemilihan model pada session aktif, tetapi ditampilkan agar pengguna tahu default belum tersimpan. Kesalahan config utama saat berpindah session mempertahankan session dan draft saat ini.

Verifikasi **19 November 2026**: **76 unit + 12 integration tests (88 total)** lulus pada macOS arm64 dan Linux aarch64, termasuk penulisan bersamaan, permission privat, preferensi rusak/tidak valid, kegagalan penyimpanan, disconnect, draft, serta prioritas model session dengan konteks opaque. Formatting dan Clippy lulus di macOS; release build dan acceptance PTY lulus pada kedua platform. PTY memakai provider mock dan memeriksa restart, dua proses pada proyek berbeda, `/new` yang membaca pilihan terbaru, dan resume yang tidak menimpa default maupun memulai inference. Bukti: [macOS](../validation/global-model-macos-arm64.json), [Linux](../validation/global-model-linux-arm64.json). Tidak ada dependency baru atau pengukuran resource ulang.

## Kontrak field konfigurasi

Nama berikut merupakan spesifikasi pengaturan, bukan file config yang dibuat pada tahap dokumentasi.

| Field | Default / aturan | Efek |
| --- | --- | --- |
| `schema_version` | 1 | Versi format config. Versi lebih baru ditolak secara eksplisit. |
| `agent.max_turns` | 30; integer ≥0 | 0 berarti tanpa batas putaran. |
| `ui.viewport_rows` | 6; integer u16; deprecated | Diterima untuk kompatibilitas, diabaikan oleh fullscreen. |
| `ui.max_fps` | 30; rentang 1–60 | Batas redraw; tidak mengubah kecepatan pencatatan hasil. |
| `tools.max_file_bytes` | 1 MiB; positif | Batas built-in read/patch teks. |
| `tools.read_max_lines` | 200; positif | Batas per hasil read; pagination tetap tersedia. |
| `tools.result_max_bytes` | 64 KiB; positif | Batas excerpt teks yang diberikan sebagai hasil tool. |
| `tools.list_page_size` | 100; positif | Ukuran halaman list/search. |
| `shell.executable` | `/bin/sh`; absolute executable path | Shell non-interaktif. |
| `shell.timeout_seconds` | 120; positif | Timeout default command yang diminta. |
| `shell.max_timeout_seconds` | 600; ≥default timeout | Maksimum timeout yang dapat diminta satu call. |
| `network.header_timeout_seconds` | 60; positif | Waktu menunggu header response. |
| `network.chunk_timeout_seconds` | 120; positif | Maksimum jeda antar-event stream. |
| `network.request_timeout_seconds` | 600; positif | Batas keseluruhan satu request generasi. |
| `providers.<id>` | ID unik dan stabil | Profil, protokol, endpoint, auth reference, dan model inventory. |
| `providers.<id>.models.<id>.context_limit` | Opsional, integer positif | Limit eksplisit jika tidak tersedia dari metadata. |
| `providers.<id>.models.<id>.output_reserve_tokens` | 4096; harus lebih kecil dari limit yang diketahui | Cadangan estimasi konteks; bukan parameter wire universal. |
| `providers.<id>.models.<id>.reasoning_effort` | Opsional: none/minimal/low/medium/high/xhigh/max | Level aktif; field tidak ada berarti default provider. Harus masuk kapabilitas model. |
| `providers.<id>.models.<id>.reasoning_efforts` | Opsional: daftar unik level | Kapabilitas tersimpan; kosong berarti tanpa kontrol, tidak ada memakai metadata/preset bila tersedia. |
| `providers.<id>.models.<id>.supports_tools` | Wajib eksplisit untuk model custom | Menentukan kelayakan sebagai coding agent. |

Untuk model kecil, pengguna menetapkan reserve yang sesuai; konfigurasi reserve ≥context limit ditolak. Estimasi ukuran konteks tidak dijanjikan identik dengan tokenizer provider. Deteksi penuh tetap dapat berasal dari error provider.

Default mode session baru, persetujuan shell per command, dan larangan mutasi Plan adalah aturan produk v1. Tidak ada config untuk menonaktifkan aturan tersebut. Batas resource tidak dijadikan knob yang memotong respons tanpa pemberitahuan.

## Profil provider

Setiap profil menyimpan ID lokal, label, jenis provider, protokol, base URL bila custom, auth method, credential reference atau nama environment variable, serta daftar model yang dapat dipilih. API key tidak diletakkan sebagai string literal di config utama.

| Jenis auth | Penyimpanan/resolusi |
| --- | --- |
| API key saved | Credential reference mengarah ke record privat; `/connect` memasukkannya melalui input tersembunyi. |
| API key environment | Profil menyimpan nama variable, bukan nilainya. Jika variable kosong/hilang, koneksi belum siap; tidak beralih ke saved key otomatis. |
| ChatGPT OAuth | Record privat memuat identitas terverifikasi, registrasi, token, scope, dan expiry. |
| None | Hanya untuk endpoint loopback custom yang dipilih eksplisit. |

Satu profil memilih satu metode resolusi aktif. Mengubah metode dilakukan eksplisit melalui `/connect`. Token access/refresh tidak dipertukarkan antarprofil hanya karena email atau endpoint tampak sama. Detail identitas mengikuti [auth](04-providers-and-auth.md).

Daftar model custom memuat ID wire, label tampilan, dukungan tools/streaming, dan limit bila diketahui. Header auth standar dihasilkan auth adapter. Arbitrary header/body override dan plugin transport tidak masuk v1.

Implementasi memakai `kind`, `base_url`, `auth.method`, dan `models` pada setiap profil. Contoh config yang dapat dipakai ada di [README proyek](../../README.md). Jalur data privat ruvx dilindungi secara internal dari file tools dan snapshot; daftar tersebut tidak dapat diperluas atau dinonaktifkan melalui config repo.

## Struktur session

| Artefak | Isi dan perannya |
| --- | --- |
| Jurnal `events.jsonl` | Urutan event session yang menjadi sumber state utama. |
| Metadata turunan | Label, waktu, workspace, status terakhir; dapat dibangun ulang dari jurnal. |
| Artefak respons/tool | Konten panjang dengan ID referensi dan hash; tidak seluruhnya dimuat saat startup. |
| Baseline file | Kondisi sebelum perubahan pertama yang dicakup review session. |
| Versi hasil file | Kondisi sesudah perubahan yang dicatat agar divergence disk dapat dikenali. |
| Recovery copy | Prefix jurnal valid bila tail terpotong; berkas asli tetap dipertahankan. |

ID session harus unik, tidak berasal dari prompt pengguna, dan tidak menyisipkan path proyek. Workspace disimpan sebagai canonical absolute path; index memakai key turunan path untuk lookup. Cache index tidak menentukan kebenaran tindakan.

## Envelope jurnal

Setiap event persisten memiliki `schema_version`, `event_id`, `sequence`, `timestamp`, `session_id`, `kind`, dan payload yang sesuai jenisnya. `run_id`, `turn_id`, serta `tool_call_id` ada ketika relevan. Urutan lokal menggunakan sequence yang meningkat; timestamp UTC bukan pengganti urutan.

| Kelompok event | Informasi yang dibutuhkan |
| --- | --- |
| Session created/recovered | Workspace, versi format, sumber recovery bila ada. |
| User message | Teks asli, referensi file, metadata pengiriman. |
| Mode/model selected | Pilihan lama/baru dan intent pengguna. |
| Run started/stopped | Batas putaran, jumlah terpakai, alasan berhenti. |
| Response completed/interrupted | Profil/model, konten atau artefak, tool calls lengkap, usage bila tersedia. |
| Tool intent/started/finished | Argumen tervalidasi, prasyarat, ID approval bila shell, status dan artefak hasil. |
| Approval resolved | Pilihan pengguna yang terikat command/cwd/timeout. |
| Context compacted | Rentang sumber, ringkasan, versi konteks baru. |

Data autentikasi tidak menjadi payload event. Diagnostik tidak mencetak headers auth, token response, atau isi credentials file. Output shell merupakan data perintah yang disetujui pengguna dan dapat memuat isi proyek; tidak dijanjikan memiliki detektor semua rahasia arbitrer.

## Durability, locking, dan error

Intent mutasi serta approval shell harus tersimpan dan disinkronkan sebelum eksekusi dimulai. Hasil final disimpan setelah tindakan selesai. Ini menyediakan jejak untuk recovery, bukan transaksi atomik antara filesystem proyek dan jurnal.

Gunakan lock writer per workspace/session dan lock refresh per registrasi auth. Lock OS dilepas ketika proses mati; PID file saja tidak cukup untuk memutuskan lock masih aktif. Pembacaan daftar session tidak membutuhkan membuka writer baru.

Jika penyimpanan gagal, hentikan tindakan baru dan pertahankan informasi yang masih ada di memori untuk ditampilkan. Jangan menyatakan autosave berhasil atau memulai mutasi berikutnya. Pengguna diberi path yang bermasalah dan opsi keluar setelah melihat status; tidak ada penghapusan data otomatis untuk mengatasi disk penuh.

V1 menulis format versi 1. Migrasi format versi mendatang dilakukan sebagai pekerjaan terpisah dengan backup dan pengujian kompatibilitas. Reader tidak menimpa versi yang tidak dikenalnya.

Panduan effort dan prioritas metadata/preset terdapat pada [effort per model](../model-efforts.md). Preferensi tersimpan per profil/model dan memengaruhi seluruh putaran tool serta compaction manual.
