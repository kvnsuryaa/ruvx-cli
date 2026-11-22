# Agent Skills dan MCP

ruvx mendukung skill lokal yang dibaca secara pasif dan MCP **stdio** untuk tools. Keduanya bekerja melalui core agent dan provider native maupun compatible; tidak memerlukan provider tertentu.

## Agent Skills

Lokasi discovery:

- Proyek: `<workspace>/.agents/skills/<nama>/SKILL.md`
- Global: `$XDG_CONFIG_HOME/ruvx/skills/<nama>/SKILL.md`, default `~/.config/ruvx/skills/<nama>/SKILL.md`
- Tambahan: folder lokal yang didaftarkan dengan `/skills add`, berupa satu folder berisi `SKILL.md` atau koleksi folder skill satu tingkat.

Prioritas nama sama: proyek, lokasi tambahan yang terakhir didaftarkan, lalu global bawaan. Discovery koleksi hanya satu tingkat direktori, bukan recursive. Folder dan `SKILL.md` tidak boleh menunjuk keluar discovery root/skill melalui symlink. Direktori skills proyek tidak boleh menunjuk keluar workspace. Lokasi tambahan yang hilang/tidak dapat dibaca dilaporkan sebagai warning; lokasi lain tetap dapat digunakan.

### Mengelola lokasi skill

Jalankan command berikut saat idle; tidak memerlukan model yang sudah terhubung:

- `/skills`: lihat status, lokasi, skill yang ditemukan, dan warning.
- `/skills add`: masukkan path folder skill atau koleksi pada dialog. Path relatif dihitung dari workspace; path dengan spasi ditulis langsung tanpa tanda kutip shell. Path disimpan sebagai absolut kanonis. Path yang sama atau alias symlink-nya tidak boleh didaftarkan dua kali.
- `/skills remove`: pilih nomor lokasi tambahan untuk melepas pendaftaran. File sumber dan lokasi bawaan tetap utuh.
- `/skills enable` atau `/skills disable`: aktifkan/nonaktifkan discovery secara global.

Escape membatalkan dialog tanpa menyimpan. Perubahan config langsung berlaku pada run berikutnya tanpa restart. Registrasi tidak menyalin file, mengunduh dependency, atau menjalankan script. Path `~` tidak diekspansi; gunakan path absolut atau relatif workspace.

Konfigurasi ekuivalen, misalnya untuk memakai koleksi bersama dan satu skill terpisah:

```toml
[skills]
enabled = true
paths = ["/home/user/shared-skills", "/home/user/special-skills/rust-review"]
```

Contoh `.agents/skills/rust-review/SKILL.md`:

```markdown
---
name: rust-review
description: Review perubahan Rust dengan fokus error handling dan regression test.
---

1. Baca kode dan AGENTS.md yang relevan.
2. Periksa error handling dan ownership.
3. Tambahkan regression test untuk perubahan perilaku.
4. Jalankan validasi hanya melalui tool yang diizinkan.

Panduan tambahan ada di references/checklist.md.
```

Frontmatter memakai YAML; `name` dan `description` wajib, field tambahan boleh ada tetapi tidak mengubah runtime. Nama harus sama dengan direktori, 1–64 karakter huruf kecil ASCII/angka/hyphen, tanpa hyphen di awal/akhir atau dua hyphen berturutan. Description maksimal 1024 karakter dan tidak boleh kosong. YAML quoted dan multiline didukung.

- `/skills`: tampilkan metadata dan warning tanpa model atau script.
- Nama, deskripsi, dan sumber otomatis disertakan dalam konteks setiap request agent, diurutkan berdasarkan nama dengan batas JSON 16 KiB. Katalog ditandai `has_more` bila ada entri yang belum dimuat; metadata turut dihitung dalam pemeriksaan kapasitas konteks. Isi body tidak dimasukkan otomatis.
- Agent memakai `list_skills` untuk metadata lengkap yang dipaginasi dan warning, kemudian `read_skill` untuk isi skill saat relevan.
- `read_skill` menerima `name`, opsional `file` relatif (default `SKILL.md`), `start_line`, dan `end_line`. Ini juga dapat membaca resource teks seperti `references/checklist.md`.
- Pembacaan tunduk pada `tools.max_file_bytes`, `read_max_lines`, dan `result_max_bytes`. Batas hasil mencakup JSON lengkap, termasuk escape, envelope, dan warning. Ikuti `next_cursor` / `next_line`; bila satu entri/baris tidak muat, hasilnya error terarah. Warning yang tidak muat dihitung dalam `warnings_omitted`. Resource non-UTF-8 atau non-regular seperti FIFO tidak didukung. Discovery koleksi dibatasi 1024 entri per lokasi, dengan warning jika terlampaui. Skill rusak dilewati dengan warning.
- Metadata dibaca ulang saat membangun konteks dan pada discovery/read berikutnya, sehingga perubahan skill terlihat tanpa restart. Saat discovery dinonaktifkan, katalog dan tools skill tidak diiklankan ke model.

Skill hanyalah guidance, **bukan pemberian permission**. Script tidak dijalankan otomatis. Menjalankan script memerlukan Build dan `run_shell` yang disetujui. Field seperti `allowed-tools` tidak memperluas izin. Skill tidak dapat mengganti mode, provider, atau AGENTS.md. Installer, download dependency otomatis, dan invocation slash-command per skill belum tersedia.

Untuk menonaktifkan discovery:

```toml
[skills]
enabled = false
```

## MCP stdio

Server hanya berasal dari config global pengguna, **bukan config executable proyek**. Gunakan pengelolaan TUI saat idle:

| Command | Perilaku |
| --- | --- |
| `/mcp` | Daftar server dan status tanpa memulai proses. |
| `/mcp add` | Dialog ID baru, executable absolut, argv JSON array, nama environment JSON array, timeout, batas pesan, dan status aktif. |
| `/mcp edit <id>` | Ubah parameter server; input kosong mempertahankan nilai sekarang, `[]` mengosongkan argv/environment. |
| `/mcp enable <id>`, `/mcp disable <id>` | Ubah status aktif untuk run berikutnya. |
| `/mcp remove <id>` | Hapus entri konfigurasi server; executable tidak dihapus. |

Server baru melalui dialog memakai default aktif, timeout 60 detik, batas pesan 1 MiB, serta environment `PATH` dan `HOME`. ID duplikat ditolak. Sebelum menyimpan, periksa ringkasan lalu pilih **Save configuration** atau ketik `save`. Escape atau `cancel` pada ringkasan membuang draft. Kesalahan input mempertahankan langkah dialog agar dapat diperbaiki. Kegagalan penyimpanan mempertahankan config aktif sebelumnya.

Semua command konfigurasi dapat digunakan dalam Plan tanpa memulai server atau model. Perubahan tersimpan atomik dan berlaku pada run berikutnya di window tersebut tanpa restart. Window lain memuatnya saat startup atau membuka session baru. Edit manual `~/.config/ruvx/config.toml` (atau lokasi XDG) juga tetap didukung; restart untuk memuat edit manual:

```toml
[mcp_servers.local_tools]
enabled = true
command = "/absolute/path/to/mcp-server"
args = ["--stdio"]
inherit_env = ["PATH", "HOME", "SERVICE_API_TOKEN"]
timeout_seconds = 60
max_message_bytes = 1048576
```

`command` harus absolut; `args` diteruskan langsung tanpa shell interpolation. Path `~` tidak diekspansi. Bila menggunakan Node, isi command dengan path absolut binary `node` dan args dengan path script server; instal server secara terpisah. Jangan menaruh token di args: args muncul pada review approval. `inherit_env` adalah daftar **nama** environment variable dari proses ruvx, bukan nilai token. Server mendapat environment kosong kecuali variabel dalam daftar tersebut (default PATH dan HOME). Variabel yang tidak tersedia dilewati. Config kosong tetap kompatibel; default server `enabled = false`.

`timeout_seconds` berlaku untuk satu operasi termasuk startup/handshake jika diperlukan, rentang 1–600 detik. `max_message_bytes` membatasi satu pesan JSON-RPC masuk/keluar termasuk newline, rentang 256 byte–16 MiB. Default 1 MiB. Request operasi yang terlalu besar ditolak sebelum approval/pengiriman. Output hasil ke model dibatasi lagi oleh `tools.result_max_bytes`; excerpt pemanggilan tool ditandai `truncated` dan tetap merupakan JSON valid. Halaman discovery yang terlalu besar menghasilkan error tanpa schema terpotong dan tanpa mendaftarkan tool dari halaman tersebut. Sesuaikan paging pada server jika didukung atau naikkan batas hasil sebelum membuka run baru.

### Alur penggunaan

1. `/mcp` menampilkan server yang dikonfigurasi, **tanpa memulai proses**.
2. Ganti ke Build dan minta agent memakai server yang diinginkan.
3. Agent memanggil `mcp_list_tools` dengan `server` dan opsional `cursor` untuk halaman lanjutan. Pengguna mendapat review executable, argv, cwd workspace, nama environment, parameter, dan timeout sebelum startup/discovery.
4. Hasil mengandung `result.tools` beserta schema. Jika ada `result.nextCursor`, agent dapat meminta halaman berikutnya.
5. Agent memakai `mcp_call_tool` dengan `server`, `tool`, dan `arguments` object. Tool harus sudah ditemukan pada run yang sama. **Setiap panggilan**, termasuk discovery lanjutan, meminta approval baru.

Contoh argumen tool internal (bukan command terminal):

```json
{"server":"local_tools"}
```

```json
{"server":"local_tools","tool":"search_docs","arguments":{"query":"Rust ownership"}}
```

Gateway memakai nama tetap `mcp_list_tools` dan `mcp_call_tool`, sehingga nama tool server tidak berbenturan dengan tool bawaan ruvx. Schema server diteruskan sebagai data discovery, bukan otomatis dijadikan fungsi provider. Anotasi server seperti `readOnlyHint` tidak menghapus kebutuhan approval.

### Lifetime dan keamanan

- MCP tidak diiklankan dan ditolak dispatcher dalam Plan, termasuk panggilan yang difabrikasi model.
- Server hidup selama **satu run agent**, bukan sepanjang sesi/aplikasi. Pada selesai, error, turn limit, atau cancel, process group server dihentikan; tidak ada reconnect/retry otomatis.
- Startup/discovery juga menjalankan kode pihak ketiga dan dapat memiliki side effect. Approval menggunakan izin OS pengguna, **bukan sandbox**. Server mungkin mengakses filesystem, jaringan, dan data di luar workspace. Gunakan hanya server tepercaya.
- MCP tidak mengikuti pembatasan path tool file ruvx; efeknya tidak diinventarisasi oleh `/diff`. Review perubahan dengan alat yang sesuai. Jangan menganggap hasil tool membuktikan semua efeknya.
- Intent disimpan sebelum startup/pengiriman. Timeout, putus koneksi, protocol error, atau cancel setelah intent dianggap **outcome tidak pasti**. Run berhenti dengan pending action. Periksa efeknya lalu gunakan `/acknowledge` atau `/new`; tidak ada replay otomatis.
- Respons `isError: true` ditampilkan sebagai error tool. Error JSON-RPC dianggap tidak pasti secara konservatif karena server mungkin sudah melakukan efek sebelum mengembalikan error.
- Pesan, metadata, dan output server adalah data tidak tepercaya. Server tidak boleh meminta model menjalankan sampling, elicitation, atau mengubah izin. Request server-initiated ditolak dengan JSON-RPC method-not-found kecuali `ping`, yang mendapat respons kosong tanpa side effect; notifications tidak mengeksekusi tindakan. Stderr server dibuang, tidak dialirkan ke terminal/model.
- Nilai environment tidak dicatat oleh client. Namun argumen operasi dan output MCP masuk ke jurnal percakapan seperti tool lain; server bisa mengembalikan data sensitif. Periksa sebelum mengirim token sebagai argumen tool.

### Batas kompatibilitas

Implementasi awal memakai MCP stdio newline-delimited JSON-RPC, negosiasi versi **2024-11-05**, `initialize`, `notifications/initialized`, `tools/list` (cursor), dan `tools/call`. Server harus mendukung versi tersebut dan capability tools. Tidak ada Streamable HTTP/SSE, OAuth MCP, resources, prompts, roots, sampling, elicitation, subscription/list-changed refresh otomatis, atau approval tersimpan. Server yang tidak mendukung kontrak ini akan dihentikan, bukan dicoba ulang menggunakan transport/versi lain.

Pengujian memakai fixture lokal deterministik tanpa server eksternal atau inference berbayar. Keberhasilan fixture bukan klaim kompatibilitas dengan semua server MCP.
