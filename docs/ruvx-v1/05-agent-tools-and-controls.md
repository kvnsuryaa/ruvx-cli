# Agent Tools dan Kendali Eksekusi

Agent ruvx bekerja melalui lima tools bawaan. Model meminta tindakan; dispatcher menentukan apakah tindakan valid dan diizinkan. Aturan mode dan persetujuan tetap berlaku walaupun instruksi repo, respons provider, atau hasil tool meminta sebaliknya.

Ekstensi menambahkan `list_skills`/`read_skill` untuk skill lokal pasif serta `mcp_list_tools`/`mcp_call_tool` untuk MCP stdio. Metadata skill masuk ke konteks secara otomatis dengan batas 16 KiB; body/resource dibaca sesuai kebutuhan. MCP hanya tersedia dalam Build dan secara default setiap operasi, termasuk startup/discovery, memerlukan approval. Pengguna dapat memilih BYPASS untuk melewati approval shell/MCP selama proses aktif. Pengelolaan `/skills` dan `/mcp` saat idle hanya mengubah konfigurasi atas tindakan pengguna; tidak menjalankan model atau server. Lihat [panduan Agent Skills dan MCP](../skills-and-mcp.md) untuk command dan kontrak lengkap.

## Siklus satu run

1. Catat pesan pengguna, mode, profil/model, dan batas putaran pada awal run.
2. Susun konteks serta daftar tools yang sesuai mode.
3. Kirim satu request generasi dan tampilkan streaming tanpa mengeksekusi fragmen tool.
4. Setelah completion berhasil, validasi seluruh tool calls dan argumennya.
5. Jalankan calls berurutan dengan pemeriksaan mode dan approval yang dibutuhkan.
6. Simpan hasil, lalu lanjutkan model jika batas run belum tercapai.
7. Tutup run ketika respons selesai tanpa tools; pause ketika error, cancel, kapasitas konteks, atau batas putaran menghentikannya.

Default `agent.max_turns` adalah 30. Nilai 0 berarti tanpa batas jumlah putaran. Pembatasan mode, timeout, kapasitas konteks, cancel, dan persetujuan shell tetap berlaku dalam mode tanpa batas.

Satu putaran dihitung ketika request generasi mulai dikirim. Model discovery, login, dan eksekusi tool tidak menambah penghitung. Compaction manual merupakan operasi tersendiri di luar run. Saat batas tercapai setelah tool calls, selesaikan calls dari completion yang sudah diterima beserta hasilnya, lalu pause sebelum request berikutnya. `/continue` memulai budget run baru dengan konteks yang sama.

## Matriks izin

| Tindakan | Plan | Build |
| --- | --- | --- |
| List, search, read | Langsung | Langsung |
| Metadata dan pembacaan skill lokal | Langsung | Langsung |
| Operasi MCP, termasuk discovery | Ditolak | Approval per operasi pada Ask; langsung pada BYPASS |
| Patch file proyek | Ditolak | Langsung setelah validasi |
| Shell apa pun | Ditolak | Approval per command pada Ask; langsung pada BYPASS |
| Mengubah mode | Hanya intent pengguna saat idle | Hanya intent pengguna saat idle |
| Menulis metadata aplikasi | Diizinkan oleh operasi aplikasi | Diizinkan oleh operasi aplikasi |

Tidak ada pengecualian shell "read-only" dalam Plan pada v1. Pencarian repo dilakukan dengan tools bawaan, bukan meminta model menjalankan shell pencarian.

## Kontrak tools

Nama dan kolom berikut adalah spesifikasi interface, belum source code atau schema executable. Respons setiap tool memuat call ID, status, ringkasan, data yang dibatasi, dan penanda truncation bila ada.

| Tool | Masukan minimum | Keluaran utama | Kondisi gagal |
| --- | --- | --- | --- |
| `list_files` | Path relatif opsional, filter path, cursor | Path relatif, jenis entri, cursor berikutnya | Path tidak valid, keluar workspace, izin OS |
| `search_text` | Query literal, path/glob opsional, cursor | Path, nomor baris, excerpt, cursor | Batas ukuran, encoding, izin OS |
| `read_file` | Path relatif, rentang baris opsional | Isi teks, rentang aktual, total jika tersedia, hash versi file | File hilang, binary, terlalu besar, keluar workspace |
| `apply_patch` | Satu file, operasi add/update/delete, perubahan teks, expected hash atau expected absent | Hasil perubahan, hash baru, diff summary | Mode, konflik versi, symlink keluar, patch tidak cocok |
| `run_shell` | Command, cwd relatif opsional, timeout opsional | Exit code/signal, stdout/stderr excerpt, durasi, referensi log | Mode, penolakan approval, timeout, spawn error |

Search literal dipilih untuk v1 agar kontrak kecil dan dapat diprediksi. Regex dan expression engine tidak wajib. Paging dan glob path menyediakan cara mempersempit hasil tanpa membangun indeks repo.

## Batas workspace

Workspace adalah direktori kanonis yang sudah ada saat session dibuka. Tools file menerima path relatif; path absolut, traversal keluar root, serta symlink menuju luar root ditolak. Untuk file baru, validasi parent kanonis dan jangan mengikuti symlink yang mengubah tujuan keluar workspace.

List/search mengikuti `.gitignore` secara default dan melewati direktori `.git`. File yang diabaikan boleh dibaca bila pengguna menyebut path-nya secara eksplisit melalui referensi file; akses tetap dibatasi workspace. Instruksi yang diperoleh model tidak dapat memperluas root.

Shell dijalankan dengan cwd di workspace, tetapi memakai izin OS proses ruvx. Pemeriksaan cwd bukan sandbox filesystem atau network. V1 tidak menyatakan bahwa shell yang disetujui mustahil menyentuh lokasi lain; command lengkap ditampilkan untuk keputusan pengguna. Review session hanya mencakup workspace.

## Batas data default

| Batas teknis | Nilai default | Perilaku ketika tercapai |
| --- | --- | --- |
| Isi teks yang dibaca per hasil | 200 baris dan maksimal 64 KiB | Kembalikan rentang serta cara melanjutkan. |
| Ukuran file untuk built-in text read/patch | 1 MiB | Error terarah; jangan memasukkan file seluruhnya ke konteks. |
| Hasil list/search per halaman | 100 entri dan maksimal 64 KiB | Kembalikan cursor. |
| Excerpt hasil shell ke model | Maksimal 64 KiB total | Head/tail bertanda truncation; hasil lengkap di artefak lokal. |
| Tail stdout/stderr di UI | Maksimal 64 KiB | Gulir tail; jangan menahan seluruh output di RAM. |
| Timeout shell default | 120 detik | Hentikan process group dan laporkan timeout. |
| Timeout shell maksimum per call | 600 detik | Tolak nilai lebih besar; pengguna dapat mengubah konfigurasi global dahulu. |

Nilai ini adalah default teknis yang dapat dikonfigurasi, bukan keputusan angka dari diskusi pengguna. Pembacaan sebagian harus menjelaskan bahwa konten belum lengkap. Binary dan file non-UTF-8 tidak diubah oleh patch tool v1.

## Penerapan patch dan konflik

`read_file` mengembalikan hash konten agar patch update/delete memerlukan versi yang jelas. Add memerlukan file belum ada. Tepat sebelum mengganti file, executor memeriksa ulang prasyarat; perubahan dari editor lain menghasilkan conflict dan mengharuskan agent membaca versi baru.

Patch update menggunakan penggantian file atomik bila platform mendukung; permission file yang ada dipertahankan. Penulisan tidak pernah dianggap berhasil hanya karena model menyatakan selesai. Operasi satu tool memengaruhi satu file agar kegagalan multi-file tidak disamarkan sebagai transaksi atomik.

Sebelum edit pertama session terhadap satu path, simpan baseline serta hash-nya. Untuk file baru, baseline adalah absent. Sesudah setiap edit simpan versi hasil dan hash. Baseline dipakai untuk review, bukan fitur undo. Jika file kemudian diubah di luar session, diff menandai divergence dan tidak mengklaim perubahan eksternal sebagai karya agent.

Delete file diperbolehkan di Build dengan prasyarat hash yang sama. Penghapusan direktori atau perubahan binary memerlukan shell yang disetujui, bukan patch tool.

## Shell dan approval

Default executor menggunakan `/bin/sh` non-interaktif dengan command string dan cwd yang disetujui. Environment proses diwariskan seperti aplikasi lokal biasa; nilai environment tidak dicetak ke log konfigurasi. Shell path dapat diubah lewat konfigurasi pengguna.

Setiap shell memulai session OS baru dengan `setsid`, sehingga `/dev/tty` tidak menunjuk ke terminal TUI; process group untuk cancel/timeout tetap memakai PID shell. `GIT_TERMINAL_PROMPT=0` menonaktifkan prompt username/password Git pada terminal. Kebutuhan kredensial dilaporkan melalui stderr hasil tool dan `auth_hint` bila pesan Git menyebut terminal prompts disabled. Autentikasi dilakukan di terminal terpisah; credential helper dan SSH agent tetap diwariskan. Isolasi terminal ini tidak mengubah izin filesystem/network dan bukan sandbox OS.

Pada kebijakan Ask bawaan, approval mengikat call ID, command, cwd, dan timeout. Pengguna memilih jalankan sekali atau tolak. Jika argumen berubah, diperlukan approval baru. Tidak ada cache izin, allowlist per session, atau persetujuan yang diwariskan dari tool call sebelumnya pada v1.

`--bypass` atau `/bypass on` merupakan pilihan eksplisit pengguna untuk masuk Build dan menjalankan shell/MCP tanpa approval per operasi. `/bypass off` mengaktifkan kembali Ask. Pilihan hanya berada dalam memori proses, ditandai pada header dan jurnal, serta tidak dimuat/disimpan dari TOML. Pergantian session dalam proses yang sama mempertahankan pilihan; restart/resume tanpa flag kembali Ask. Mode Plan tetap menolak shell/MCP, dan timeout/cancel/validasi serta larangan replay outcome tidak pasti tetap berlaku.

Output disalurkan ke UI sebagai event dan ke artefak lokal secara bertahap. Teks yang ditampilkan melewati penyaringan escape sequence terminal agar stdout tool tidak mengambil alih renderer. Exit code bukan nol menjadi hasil tool gagal dengan excerpt yang relevan; model dapat menyesuaikan langkah berikutnya, tetap dalam batas run.

### Baseline perubahan shell

Sebelum menampilkan approval, ambil snapshot teks workspace yang memenuhi cakupan review. Snapshot tidak memuat `.git`, direktori dependensi/build umum (`node_modules`, `target`, `.venv`), path yang diabaikan, symlink keluar, binary, atau file di atas batas built-in text. Simpan isi eligible file ke disk secara bertahap, bukan seluruh workspace di RAM. Tampilkan jumlah file dan pengecualian cakupan bersama review command.

Sesudah shell berhenti, bandingkan snapshot sebelum/sesudah. File yang berubah dan belum memiliki baseline session memakai keadaan sebelum command sebagai baseline; file baru memakai absent. Baseline yang sudah ada dari edit sebelumnya tetap dipertahankan. Hasil snapshot juga menyimpan versi terbaru untuk deteksi perubahan eksternal berikutnya.

Jika persiapan snapshot gagal, jangan memulai command dengan klaim review lengkap; tampilkan error dan tunggu perbaikan/pembatalan. Snapshot yang lama tertunda sebelum approval harus divalidasi ulang sebelum spawn. Perubahan di antara snapshot dan spawn memerlukan persiapan serta approval baru.

File yang berubah selama interval shell tidak selalu membuktikan penulisnya, karena editor lain masih dapat bekerja. Tandai sumber sebagai perubahan yang teramati selama command, bukan atribusi pasti. File di luar cakupan snapshot hanya muncul sebagai keterbatasan review, tanpa diff yang diklaim lengkap. Snapshot adalah dasar review dan bukan fitur rollback.

Ctrl+C dan timeout menghentikan process group: kirim terminasi, tunggu maksimal 2 detik, kemudian kill bila masih hidup. Hasil harus membedakan exited, denied, canceled, timed out, dan spawn failed. Proses yang terlepas dari process group tidak dijanjikan dapat dipulihkan; background process bukan workflow v1.

## Validasi dan kegagalan

Tool call yang belum lengkap, JSON tidak valid, nama tidak dikenal, atau schema salah tidak dijalankan. Error terstruktur boleh dikembalikan kepada model untuk koreksi pada putaran berikutnya. Respons stream yang gagal sebelum completion tidak menghasilkan eksekusi tools dari fragmen yang sempat diterima.

Dispatcher memeriksa aturan di depan executor, bukan hanya menyembunyikan tools dari prompt. Pengujian harus mencoba calls terlarang secara langsung. Saat state store gagal sebelum intent mutasi tercatat, jangan mulai mutasi. Jika error terjadi setelah tindakan, pause dan laporkan hasil yang diketahui tanpa rollback atau retry otomatis.

Implementasi juga membandingkan hash AGENTS.md pada scope target sebelum patch/shell terhadap instruksi yang sudah diberikan ke model. Scope yang baru ditemukan atau berubah menghasilkan `instructions_required` tanpa menjalankan tindakan; model menerima instruksi tersebut dan dapat mencoba lagi pada putaran berikutnya. Patch update menerima penggantian isi penuh melalui `content`, atau pasangan `old_text`/`new_text` yang harus cocok tepat satu kali, beserta `expected_hash`.

## Ringkasan akhir run

Agent diminta menjelaskan file yang berubah, alasan perubahan, validasi yang dijalankan dan hasil aktualnya, serta hal yang masih perlu dilakukan. UI menyediakan `/diff` terpisah dari klaim model. Summary tidak boleh menyebut tests passed bila tidak ada hasil eksekusi yang mendukungnya.
