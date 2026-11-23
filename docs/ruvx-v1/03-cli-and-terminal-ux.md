# CLI dan Pengalaman Terminal ruvx v1

ruvx menggunakan TUI fullscreen berfokus pada percakapan, dengan header status dan composer multiline. Mouse capture aktif untuk scroll, klik kontrol, dan drag seleksi. Tidak ada sidebar permanen atau mode inline kedua.

## Titik masuk

| Antarmuka | Perilaku yang dirancang |
| --- | --- |
| `ruvx` | Buka session baru pada direktori kerja saat ini dalam Plan. |
| `ruvx <path>` | Buka session pada direktori yang sudah ada; direktori kosong sah untuk proyek baru. |
| `ruvx --resume <session-id>` | Pulihkan session beserta workspace-nya; jangan memulai request model otomatis. |
| `ruvx --help`, `ruvx --version` | Keluaran singkat, tanpa login atau membuka terminal interaktif. |

Path yang tidak ada menghasilkan petunjuk membuat direktori terlebih dahulu. ruvx tidak membuat workspace secara implisit ketika pengguna bermaksud merencanakan. Jika input/output bukan terminal, tampilkan bahwa mode headless belum didukung dan keluar tanpa memanggil model. Bantuan dan versi tetap dapat dipakai tanpa TTY.

Tidak ada subcommand khusus otomasi di v1. Pengelolaan akun dan session dilakukan lewat command interaktif berikut.

## Command interaktif

Mengetik `/` membuka panel command di atas composer, dengan `/help` paling awal dan maksimal enam pilihan terlihat. Prefix seperti `/m` memfilter command; Up/Down dan wheel di atas panel memilih, Enter/klik menjalankan atau membuka dialog, Tab hanya melengkapi dengan spasi, dan Escape menutup tanpa membuang input. `/mode` serta `/resume` memerlukan argumen sehingga Enter/klik mengisi prefix dan spasi terlebih dahulu. Daftar menu dan bantuan berasal dari metadata command yang sama.

Panel menutup saat ada whitespace/argumen, multiline, atau awalan `//`, dan tidak muncul pada node paste, editor paste, dialog konfigurasi, maupun approval shell. Setelah Escape, perubahan teks berikutnya boleh membuka panel kembali. Saat run aktif, panel tetap dapat dilihat dan Tab tetap melengkapi, tetapi eksekusi dinonaktifkan dengan petunjuk menunggu atau Ctrl+C. Tidak ada antrean command yang berjalan otomatis setelah run selesai.

| Command | Perilaku |
| --- | --- |
| `/help` | Daftar command dan shortcut yang tersedia. |
| `/connect [profile]` | Pilih jenis provider, profil baru, dan metode autentikasi; argumen profil ChatGPT melakukan reauthorization memakai registrasi yang disimpan. |
| `/disconnect` | Lepaskan kredensial profil yang dipilih setelah konfirmasi; tidak menghapus session. |
| `/model` | Pilih profil serta model; perubahan dicatat dalam session. |
| `/effort [default\|level]` | Pilih reasoning effort model aktif; tersedia juga selector klik/keyboard pada status. |
| `/effort levels [...]` | Atur kapabilitas effort model custom/unknown, atau buka editor daftar bila tanpa argumen. |
| `/mode plan`, `/mode build` | Ubah mode ketika tidak ada run aktif. |
| `/new` | Simpan session saat ini dan mulai session baru di workspace yang sama dalam Plan. |
| `/sessions` | Tampilkan session workspace saat ini dan pilihan resume. |
| `/resume <session-id>` | Pulihkan session di workspace yang sama ketika idle. |
| `/compact` | Minta ringkasan konteks secara eksplisit; tampilkan cakupannya sebelum mengirim request. |
| `/diff [path]` | Tampilkan diff perubahan sesi; opsional satu file relatif workspace. |
| `/continue` | Lanjutkan run yang berhenti karena batas putaran; mulai penghitung run baru. |
| `/acknowledge` | Setelah memeriksa efek tindakan terputus, konfirmasikan bahwa outcome uncertain sudah ditinjau. Tidak menyatakan sukses dan tidak mengeksekusi ulang. |
| `/quit` | Tutup session dan pulihkan terminal. |

Command yang mengubah session, mode, model, atau koneksi hanya berjalan ketika idle. Saat ada pekerjaan aktif, UI menawarkan pembatalan lebih dahulu. Command lokal tidak diteruskan sebagai prompt model. Command tidak dikenal menampilkan bantuan; teks dengan awalan `//` dikirim sebagai teks berawalan satu `/`.

## Onboarding dan koneksi

1. Tampilkan prompt dan status session segera setelah state lokal siap. Jika session belum memiliki model, pulihkan pilihan terakhir pengguna yang masih valid tanpa discovery atau request jaringan.
2. Jika belum ada koneksi, pengguna dapat menjalankan `/connect`. Prompt coding tanpa koneksi mengarahkan ke command itu tanpa membuang teks input.
3. Pengguna memilih OpenAI atau custom compatible. Untuk OpenAI tersedia login ChatGPT dan API key. Custom menggunakan API key atau tanpa kredensial bila endpoint lokal memang dikonfigurasi demikian.
4. API key dimasukkan dalam input tersembunyi atau dirujuk lewat nama environment variable. Nilainya tidak masuk scrollback maupun transcript.
5. Login ChatGPT membuka browser melalui jalur resmi. UI memperlihatkan menunggu, berhasil, dibatalkan, atau gagal; koneksi lama tetap aktif sampai koneksi baru tervalidasi.
6. Pengguna memilih model bila belum tersedia atau ingin menggantinya. Pilihan yang berhasil dicatat pada session dan menjadi default global untuk session berikutnya. Discovery yang gagal masih mengizinkan daftar model custom yang sudah dikonfigurasi, tetapi tidak mengganti model secara diam-diam.

Pilihan login mengikuti kapabilitas provider, seperti pola yang didokumentasikan [OpenCode](https://opencode.ai/docs/providers/#chatgpt). Detail kelayakan dan protokol terdapat di [provider dan auth](04-providers-and-auth.md).

## Bentuk layar

```text
ruvx · invoice-app                       [Plan] [provider/model]
ready
Conversation

  USER

  Tolong pelajari alur validasi.

  AI · ruvx

  Saya akan mencari validator dan pengujian terkait.

  read_file → ok  [Detail]

model · Plan · Shift+Tab mode · /help
> Tulis pesan...
PgUp/PgDn · wheel scroll · drag to copy · Ctrl+C cancel
```

Tampilan memakai alternate screen, raw mode, bracketed paste, dan mouse capture. Semua state terminal dipulihkan saat keluar, error/panic yang dapat ditangani, atau SIGTERM. Ukuran kecil memakai komposisi ringkas tanpa kehilangan draft; terminal sangat kecil menampilkan permintaan memperbesar layar.

Baris di atas composer menampilkan nama model dan mode aktif, memakai ID model bila nama tampilannya belum tersedia. Saat belum ada model, baris tersebut mengarahkan ke pemilihan/koneksi. Menu command, dialog, dan kontrol pemulihan konteks tetap memakai petunjuk khususnya.

Kanan baris status menampilkan `git: <branch>` jika workspace berada dalam repository Git, termasuk subfolder, worktree, dan branch yang belum memiliki commit. Detached HEAD ditampilkan sebagai `git: detached <hash-pendek>`. Proyek tanpa metadata Git yang dapat dibaca tidak menampilkan label. Pada terminal sempit, branch dipotong dengan elipsis dan tidak menutupi area klik effort.

Metadata diperiksa segera setelah UI dimulai lalu setiap dua detik melalui worker pembacaan file. UI hanya menerima pembaruan ketika nilai berubah; tidak ada proses Git, dependency baru, atau pembacaan direktori proyek secara rekursif. Pembacaan mengikuti file `.git`/`gitdir:` dan `HEAD` per worktree sebagaimana dijelaskan pada [layout repository Git](https://git-scm.com/docs/gitrepository-layout) dan [detail worktree](https://git-scm.com/docs/git-worktree#_details). Perubahan branch dari terminal lain atau shell agent akan terlihat pada siklus pemeriksaan berikutnya. Tidak ada checkout atau perubahan repository yang dilakukan oleh indikator ini.

Verifikasi indikator Git: **89 unit + 12 integration tests (101 total)**, release build, dan PTY lulus pada macOS arm64 dan Linux aarch64; format dan Clippy lulus di macOS. Tes mencakup subfolder, unborn branch, gitfile relatif/absolut, detached HEAD, metadata hilang/rusak, pembaruan hanya saat berubah, dan layout yang tidak menutupi kontrol effort. PTY kedua platform memakai repository Git sementara dan menguji pembaruan branch eksternal serta resize 48×16. Pemeriksaan tambahan macOS memakai linked worktree nyata dan checkout detached pada 100×40/48×16. Bukti: [macOS](../validation/git-branch-macos-arm64.json), [Linux](../validation/git-branch-linux-arm64.json). Pengukuran resource tidak diulang.

Transcript memuat pesan user/agent dan hasil tool. Detail tool, help, pemilihan model, dan diff terbuka sesuai kebutuhan pada area review. Review mendukung scrolling serta seleksi teks yang sama. Header menunjukkan mode, provider/model, dan status; composer menyesuaikan tinggi hingga enam baris dengan scroll internal.

Wheel dan PgUp/PgDn menggulir riwayat. Saat membaca bagian lama, pesan baru tidak menarik layar ke bawah; klik Latest atau Alt+End untuk mengikuti kembali. Resize membungkus ulang teks dengan anchor sumber. UI membaca jurnal asli secara terpisah dari proyeksi konteks model, sehingga `/compact` tidak menghilangkan riwayat tampilan. Body artefak dimuat sesuai kebutuhan lalu dibaca per bagian dari sumber file. Halaman 32 KiB dan indeks jarang setiap 128 baris visual menghindari cache seluruh pesan. Metadata indeks dan buffer sementara tetap membutuhkan memori tersendiri. Pesan user panjang dilipat secara default; jawaban AI tetap terbuka. Detail ada pada [panduan percakapan dan paste](../conversation-and-paste.md).


Seleksi memakai drag langsung di dalam aplikasi. Melepas mouse menyalin otomatis; highlight tetap sampai dibersihkan. Ctrl+C saat ada seleksi menyalin dan membersihkannya; Ctrl+C berikutnya memakai perilaku pembatalan biasa. Escape, klik baru, atau mulai mengetik membersihkan seleksi. Drag di tepi atas/bawah menggulir riwayat. Seleksi menggunakan offset teks sumber, menjaga Unicode, indentasi, dan newline asli; wrapping, label UI, dan konten tersembunyi tidak ikut tersalin.

Clipboard menggunakan helper OS yang tersedia dan fallback OSC52. ruvx menyatakan pengiriman ke terminal secara terpisah dari keberhasilan helper karena OSC52 tidak memberikan konfirmasi clipboard lokal. Jalur terminal dibatasi 100 KB; satu seleksi maksimal 16 MiB. Semua helper menerima teks lewat stdin, tanpa interpolasi shell atau instalasi otomatis. Drag pada kontrol tidak mengaktifkannya; klik baru dianggap aktif ketika mouse dilepas pada kontrol yang sama tanpa gerakan drag.

Field lama `ui.viewport_rows` diterima dan diabaikan. Tidak ada perubahan format session atau migrasi jurnal untuk fullscreen.

## Input dan shortcut

| Input | Perilaku |
| --- | --- |
| Enter | Kirim pesan atau command ketika idle; selama run teks draft tetap dapat diedit tetapi belum dikirim. |
| Alt+Enter | Tambah baris input; bracketed paste juga menerima multiline tanpa mengirim otomatis. |
| Option+Delete / Ctrl+Backspace / Ctrl+W | Hapus kata di kiri kursor beserta whitespace di belakangnya, memakai batas kata Unicode. |
| Ctrl+U | Hapus ke awal baris logis saat ini; tidak menghapus baris sebelumnya atau suffix di kanan kursor. |
| Shift+Tab | Berganti Plan/Build ketika idle, tanpa mengirim draft; pada dialog tetap navigasi pilihan. |
| F6 / Shift+F6 | Fokus kontrol berikut/sebelumnya; Tab/arrows menavigasi, Enter mengaktifkan, Escape kembali ke input. |
| Panah/Home/End | Navigasi composer; posisi mengikuti grapheme dan lebar sel terminal. |
| Up/Down | Navigasi baris composer; pada batas input satu baris dapat mengakses riwayat pesan pengguna. |
| Ctrl+C dengan seleksi | Salin seleksi dan bersihkan highlight, tanpa membatalkan agent. |
| Ctrl+C saat aktif tanpa seleksi | Batalkan request, approval, atau shell; pertahankan session dan draft. |
| Ctrl+C saat idle | Kosongkan draft bila ada; jika kosong, keluar dengan terminal dipulihkan. |
| Ctrl+D | Keluar ketika idle dan draft kosong. |
| Escape | Tutup selector atau dialog lokal; tidak menyetujui tindakan. |

Shortcut multiline dijelaskan di `/help` karena dukungan kombinasi tombol bergantung emulator terminal. F6/Shift+F6 memindahkan fokus dari composer ke kontrol terlihat; Tab/arrows menavigasi, Enter mengaktifkan, dan Escape kembali ke input. Menu pilihan juga menerima nomor/teks seperti versi sebelumnya. Seleksi OS terminal bergantung modifier emulator; alur utama ruvx memakai seleksi dalam aplikasi.

Penghapusan kata/awal baris bekerja pada composer, input dialog yang dimasking, dan editor paste panjang. Kata memakai batas Unicode; emoji dan combining marks tetap utuh. Di composer, node paste yang tersentuh dihapus secara utuh. Ctrl+U kini menghapus prefix baris saat ini; Ctrl+C saat idle tetap dapat membersihkan seluruh draft. Editor paste mencari batas melalui jendela baca bertahap, tanpa merakit ulang seluruh dokumen.

ruvx meminta pelaporan tombol yang lebih jelas melalui [keyboard protocol](https://sw.kovidgoyal.net/kitty/keyboard-protocol/#progressive-enhancement), lalu memulihkannya sebelum keluar dari alternate screen. Tidak ada query keyboard sinkron atau pembaca input tambahan. Option+Delete menerima encoding Alt+Backspace legacy maupun CSI-u. Ctrl+U adalah satu-satunya shortcut hapus ke awal baris; Command/Meta+Delete diabaikan. Jika Option ditahan terminal/OS, gunakan pengaturan Option sebagai Meta/Esc atau Ctrl+W.

Baseline shortcut edit sebelum penghapusan Command+Delete: **85 unit + 12 integration tests (97 total)**, release build, dan PTY lulus pada macOS arm64 dan Linux aarch64; format dan Clippy lulus di macOS. Tes mencakup Unicode/emoji panjang, node paste atomik dan offset node lain, prefix/suffix multiline, input masked, serta dokumen paste besar. PTY memeriksa encoding Option legacy/CSI-u, Command CSI-u, Ctrl+W/U, mode pada composer, pengeditan paste tanpa perubahan payload lain, Ctrl+C/Shift+Tab CSI-u, dan push/pop keyboard tepat sekali saat normal exit maupun SIGTERM. Pengujian encoding tidak membuktikan pemetaan tombol fisik pada setiap emulator. Bukti: [macOS](../validation/editing-shortcuts-macos-arm64.json), [Linux](../validation/editing-shortcuts-linux-arm64.json).

Penghapusan binding Command+Delete diverifikasi ulang di macOS: suite 97 tes lulus, tes composer/editor paste memastikan modifier Command/Meta tidak menghapus karakter, dan Ctrl+U tetap menghapus prefix baris. Format, Clippy, release build, dan [PTY macOS](../validation/ctrl-u-only-macos-arm64.json) lulus. Pengujian Linux di atas merupakan baseline sebelum penghapusan binding ini.

Shift+Tab pada percakapan mengganti Plan ↔ Build melalui jalur `/mode` yang sama, mempertahankan draft, dan tidak memulai tugas. Saat agent aktif pergantian ditolak; pada dialog/approval tetap berlaku navigasi pilihan, dan editor paste tidak mengganti mode. Event release/repeat shortcut diabaikan jika terminal melaporkannya. Verifikasi: **78 unit + 12 integration tests (90 total)**, release build, dan PTY macOS/Linux lulus; format dan Clippy lulus di macOS. PTY memeriksa pergantian dua arah tanpa mengirim draft dan larangan mengganti mode selama run/approval. Bukti: [macOS](../validation/mode-shortcut-macos-arm64.json), [Linux](../validation/mode-shortcut-linux-arm64.json).

## Referensi file

Composer menerima `@path` relatif workspace. Untuk path dengan spasi, gunakan bentuk `@"path dengan spasi"`. Referensi dikenali dalam pesan biasa, bukan di dalam fenced code block. File referensi dibaca dengan batas tools biasa dan dilampirkan ke konteks sebagai data, bukan instruksi sistem.

Path yang tidak ditemukan atau keluar workspace ditampilkan sebelum request model dikirim. ruvx mempertahankan draft agar pengguna bisa memperbaikinya. Autocomplete file tidak wajib untuk v1.

## Plan ke Build

Plan boleh menggunakan tools baca/cari untuk memahami proyek. Rencana muncul sebagai respons biasa dan tersimpan dalam session. Pengguna kemudian menekan Shift+Tab atau menjalankan `/mode build`; perubahan mode tidak otomatis mengirim prompt atau menjalankan rencana. Pengguna mengirim instruksi berikutnya, misalnya menerapkan rencana tersebut.

Resume memulihkan mode session yang tersimpan dan menampilkannya jelas. `/new` selalu kembali ke Plan. Model tidak dapat menekan selector atau memicu perubahan mode melalui teks.

## Persetujuan shell

Sebelum setiap eksekusi, tampilkan command lengkap, cwd, dan timeout. Command multiline atau panjang masuk ke area review yang dapat digulir; nilai yang disetujui harus identik dengan nilai yang dijalankan.

Pilihan v1 adalah **Accept** (menyetujui satu operasi) di atas **Deny**, dengan fokus awal Accept pada setiap konfirmasi shell maupun MCP. Review panjang harus digulir sampai bawah sebelum Accept dapat dijalankan. Teks `run`/`deny` tetap diterima. Timer aktivitas berhenti selama menunggu persetujuan dan berlanjut dari durasi sebelumnya setelah dijawab atau ditutup; waktu tunggu tidak ikut dihitung. Tidak ada persetujuan otomatis seluruh session. Escape atau pembatalan tidak dianggap sebagai persetujuan. Hasil penolakan dicatat dan dikembalikan ke agent sebagai tool result.

## Hasil dan kegagalan

Respons akhir membedakan perubahan yang dilakukan, validasi yang benar-benar dijalankan, hasil validasi, serta pekerjaan tersisa. Tes yang belum dijalankan tidak diberi status lulus. `/diff` menampilkan baseline perubahan session, termasuk proyek tanpa Git.

Error menampilkan tahap kegagalan dan pilihan lanjut. Batas putaran menawarkan `/continue`; konteks penuh menawarkan `/compact` atau `/new`; autentikasi kedaluwarsa menawarkan `/connect`. Tidak ada retry mutasi atau pergantian provider diam-diam.

## Kriteria UX

Pengguna harus selalu dapat mengetahui mode aktif, koneksi yang digunakan, tindakan yang sedang berjalan, apakah shell sedang menunggu izin, dan apakah proses sudah berhenti. Streaming dan keluaran shell tidak boleh mengambil alih composer atau membuat tombol approval tertekan oleh teks paste.

Effort disimpan per profil/model, tidak dapat diubah saat run aktif, dan tidak memulai request otomatis. `default` berbeda dari `none`; detail ada pada [panduan effort](../model-efforts.md).

Paste panjang memakai preview node dan editor layar penuh. Preflight berjalan di worker dengan revisi draft; gagal/cancel tidak menghilangkan draft. Tidak ada batas draft tetap, pemotongan, atau compaction otomatis.
