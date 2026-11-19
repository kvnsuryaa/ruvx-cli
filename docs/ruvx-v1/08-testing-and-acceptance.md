# Pengujian dan Penerimaan ruvx v1

Pengujian ruvx membuktikan perilaku produk pada batas subsistem, terutama ketika model, terminal, shell, atau penyimpanan gagal. Skenario di bawah menjadi matriks penerimaan. Implementasi dan tests kini tersedia; bukti yang sudah dijalankan serta cakupan yang belum diuji dicatat di [laporan verifikasi](../validation-v1.md).

## Matriks kebutuhan

| Kebutuhan | Kelompok uji | Bukti minimum |
| --- | --- | --- |
| REQ-01 | PERF-01 sampai PERF-04 | Laporan release build dengan mesin dan kondisi ukur. |
| REQ-02 | CTRL-01 sampai CTRL-04 | Calls terlarang ditolak di dispatcher; shell tidak spawn sebelum approval. |
| REQ-03 | FLOW-01 dan FLOW-02 | Siklus agent memakai tools hingga hasil tervalidasi. |
| REQ-04 | AUTH-01 sampai AUTH-05, WIRE-01 sampai WIRE-04 | Ketiga jalur koneksi diuji terpisah. |
| REQ-05 | CTX-01 dan CTX-02 | Scope instruksi benar, file dipilih sesuai kebutuhan. |
| REQ-06 | STATE-01 sampai STATE-05 | Recovery tidak menjalankan ulang tindakan. |
| REQ-07 | CTX-03 sampai CTX-05 | Kapasitas memicu jeda dan compaction hanya manual. |
| REQ-08 | DIFF-01 sampai DIFF-04 | Baseline session memisahkan perubahan awal dan perubahan sesi. |
| REQ-09 | TTY-01 sampai TTY-05 | Terminal tetap berfungsi setelah streaming, resize, cancel, dan error. |
| REQ-10 | WIRE-04, AUTH-04, STATE-04 | Error menjelaskan tahap dan tidak memicu fallback diam-diam. |
| REQ-11 | CTRL-05 dan CTRL-06 | Default 30, nilai 0, dan lanjut eksplisit. |
| REQ-12 | FLOW-01 dan FLOW-02 | Repo existing dan proyek baru sama-sama lulus. |

## Strategi verifikasi

Unit tests dipakai untuk logika murni seperti validasi argumen, penghitungan putaran, pembentukan konteks, parser delta, dan reduksi event. Integration tests memakai mock HTTP/SSE, filesystem sementara, serta proses shell deterministik. PTY tests memeriksa perilaku terminal nyata. Pengujian provider live dipisahkan agar tidak menjadi ketergantungan test suite harian.

Mock tidak membuktikan eligibility OAuth, dukungan model akun tertentu, atau angka performa nyata. Bukti live dan benchmark harus dicatat sebagai hasil berbeda. Tidak ada kebutuhan menghubungkan layanan eksternal untuk menyatakan dokumentasi ini selesai.

## Kontrol agent

| ID | Skenario | Hasil yang diharapkan |
| --- | --- | --- |
| CTRL-01 | Model memanggil patch/shell dalam Plan walau tool tidak ditawarkan | Dispatcher menolak, file tidak berubah, proses tidak dibuat. |
| CTRL-02 | Instruksi AGENTS.md meminta berpindah Build atau melewati izin | Mode/approval tetap berasal dari pengguna. |
| CTRL-03 | Build menerima beberapa shell calls | Setiap command memiliki approval terikat argumen dan dijalankan berurutan. |
| CTRL-04 | Approval ditolak, dibatalkan, atau args berubah | Tidak ada eksekusi tanpa approval yang sesuai. |
| CTRL-05 | Batas default 30 tercapai sesudah tools putaran ke-30 | Hasil tools tercatat, request ke-31 belum dikirim; `/continue` membuka budget run baru. |
| CTRL-06 | Batas 0, batas 1, dan config negatif | 0 tidak dibatasi jumlah putaran; 1 pause sesuai kontrak; negatif ditolak. |

Uji filesystem juga mencakup traversal, absolute path, symlink keluar root, file hilang, binary, batas byte/baris, serta perubahan versi antara read dan patch. Patch harus menolak konflik tanpa menimpa file baru dari editor lain.

## Wire protocol dan provider

| ID | Skenario | Hasil yang diharapkan |
| --- | --- | --- |
| WIRE-01 | SSE terpecah pada batas byte UTF-8, JSON, dan event | Parser merakit event dengan benar tanpa teks rusak. |
| WIRE-02 | Tool-call arguments datang terpotong/interleaved; ID hanya pada delta awal | Call identity dan arguments tetap tepat; eksekusi hanya sesudah completion valid. |
| WIRE-03 | Responses dan Chat Completions membawa tool call/result yang sama | Event core setara; payload wire mengikuti masing-masing protokol. |
| WIRE-04 | Timeout, koneksi putus, JSON salah, final error setelah delta | Run pause; fragmen calls tidak menjadi tindakan; tidak ganti provider otomatis. |

Tambahkan kasus body terlalu besar, unsupported tools, model hilang, discovery gagal, limit tidak diketahui, dan metadata usage absen. Nilai usage absen tidak boleh menjadi angka nol yang diklaim berasal dari provider.

## Autentikasi

| ID | Skenario | Hasil yang diharapkan |
| --- | --- | --- |
| AUTH-01 | API key saved, environment reference, dan environment hilang | Sumber key eksplisit; nilai rahasia tidak masuk transcript/diagnostik. |
| AUTH-02 | OAuth state/nonce/audience/issuer salah atau token expired | Koneksi tidak diaktifkan; attempt lama tidak ditimpa. |
| AUTH-03 | Identitas valid tetapi scope inference hilang | Status belum siap untuk inference, dengan petunjuk reauthorization. |
| AUTH-04 | Consent ditolak, refresh ditolak, kuota habis | Pesan tahap tepat; tidak pindah ke profil lain otomatis. |
| AUTH-05 | Dua proses melakukan refresh registrasi yang sama | Lock menserialkan refresh dan token rotasi tidak tertimpa versi lama. |

Periksa callback `127.0.0.1`, konsistensi URI, persistence issued client ID, file permission, atomic write, dan pemisahan akun. Jalur subscription menggunakan fixture parameter yang sesuai limit preview. Pengujian live pada akun yang memenuhi syarat membuktikan koneksi dan satu tool cycle, dengan consent pengguna serta penggunaan kuota yang dicatat pada tahap implementasi.

## Konteks dan sesi

| ID | Skenario | Hasil yang diharapkan |
| --- | --- | --- |
| CTX-01 | AGENTS.md root dan nested, termasuk instruksi bertentangan | Scope direktori benar; aturan aplikasi tidak berubah. |
| CTX-02 | Repo besar dan referensi file eksplisit | Tidak ada pemuatan seluruh repo; file tercantum dengan versi dan truncation. |
| CTX-03 | Konteks melebihi limit diketahui atau ditolak provider | Pause; tidak ada compaction atau pemotongan otomatis. |
| CTX-04 | `/compact` berhasil, gagal, atau cancel | Hanya sukses mengaktifkan ringkasan; jurnal asli tetap utuh. |
| CTX-05 | Berpindah adapter dengan opaque context yang tidak dapat diterjemahkan | Perpindahan ditahan; pengguna memilih compaction atau session baru. |
| STATE-01 | Tutup lalu resume normal | Mode, rencana, model, dan hasil tools pulih; tidak ada network request otomatis. |
| STATE-02 | Crash setelah tool intent atau sesudah spawn tanpa hasil | Tindakan uncertain ditampilkan dan tidak diputar ulang. |
| STATE-03 | Tail JSONL terpotong dan corruption di tengah | Tail memakai recovery copy; corruption di tengah menghentikan writable resume. |
| STATE-04 | Disk penuh, izin file salah, artefak hilang | Mutasi baru berhenti dan status penyimpanan tidak diklaim berhasil. |
| STATE-05 | Dua writer workspace serta versi jurnal lebih baru | Writer kedua/versi tak dikenal ditolak tanpa menimpa data. |

## Review perubahan

| ID | Skenario | Hasil yang diharapkan |
| --- | --- | --- |
| DIFF-01 | Repo sudah dirty sebelum patch pertama | Diff session hanya membandingkan baseline awal file dengan hasil session. |
| DIFF-02 | File baru, file dihapus, dan repo tanpa Git | Diff tetap tersedia dengan status yang tepat. |
| DIFF-03 | Editor lain mengubah file sesudah agent | Divergence ditandai; perubahan eksternal tidak diklaim sebagai patch agent. |
| DIFF-04 | Shell membuat/mengubah file | Snapshot sebelum/sesudah menyediakan review dalam cakupan yang dinyatakan; excluded files dan ketidakpastian penulis ditandai. |

Kegagalan snapshot sebelum shell harus menghentikan langkah persiapan; pengguna tidak boleh menyetujui command dengan klaim cakupan diff yang belum benar. Pengujian memastikan file yang sudah diubah melalui patch mempertahankan baseline session pertamanya.

## Terminal dan proses

| ID | Skenario | Hasil yang diharapkan |
| --- | --- | --- |
| TTY-01 | Respons streaming panjang dan code fence lintas delta | Tidak ada duplikasi transcript; render hanya viewport, scrolling tidak ditarik ke bawah oleh streaming. |
| TTY-02 | Resize ketika streaming, mengetik, atau approval | Composer/status tetap konsisten, command tidak tersembunyi saat keputusan dibuat. |
| TTY-03 | Emoji, combining marks, CJK, paste multiline | Cursor/delete mengikuti grapheme; paste tidak mengirim atau menyetujui otomatis. |
| TTY-04 | Ctrl+C pada network, approval, dan shell turunan | Tanpa seleksi: pekerjaan dibatalkan, process group dihentikan, hasil tetap tercatat. Dengan seleksi: salin dan bersihkan highlight dahulu. |
| TTY-05 | Exit normal, error dan panic | Raw mode/cursor/paste/mouse/alternate screen dipulihkan, shell terminal dapat digunakan kembali. |
| TTY-06 | Drag Unicode/kode melintasi viewport, resize dan streaming | Offset seleksi stabil, indentasi/newline asli tersalin, label UI tidak ikut. |
| TTY-07 | Klik/drag pada approval, Enter default, review panjang | Drag tidak menjalankan shell; default Deny; approval menunggu seluruh review terbaca. |
| TTY-08 | Scroll lama, pesan baru, resume sesudah compaction | Tidak auto-jump; Latest kembali mengikuti; jurnal asli tetap terbaca dengan lazy cache. |
| TTY-09 | Clipboard helper, OSC52, SSH dan kegagalan helper | Teks dikirim literal via stdin; payload terenkode; status tidak mengklaim konfirmasi clipboard remote. |
| TTY-10 | Peran user/AI pada live, resume dan scroll tengah pesan | Label/pembatas berbeda tanpa mengandalkan warna; heading menyebut peran lanjutan. |
| TTY-11 | Paste panjang campuran teks, edit/save/cancel/remove | Preview tidak terkirim; urutan/Unicode/indentasi lengkap diterima provider setelah Enter. |
| TTY-12 | Konteks gagal dan draft berubah saat preflight | Draft/referensi tetap utuh; tidak ada request/replay atau pesan user yang tercatat sebelum lulus. |
| TTY-13 | Draft 1 MiB/10 MiB dan transcript 10.000 pesan | Pembacaan per bagian, indeks jarang, respons editor dan RSS dilaporkan; ukuran ini bukan hard limit. |
| TTY-14 | Usage Responses/Chat Completions beserta attribution | Ringkasan aggregate tidak menghitung ganda; metadata mentah hanya tampil pada Detail. |

Jalankan PTY tests pada macOS dan Linux. Tambahkan pemeriksaan manual minimal pada terminal pengguna macOS dan satu terminal Linux. Record terminal emulator, ukuran, dan capability yang relevan. Shortcut yang tidak tersedia harus memiliki jalur alternatif via command/paste.

## Dua skenario penerimaan

### FLOW-01 Perubahan pada repo existing

Gunakan fixture repo kecil dengan tes yang sudah ada dan satu perubahan pengguna yang belum di-commit. Minta agent memahami proyek dan merencanakan fitur/perbaikan yang memerlukan beberapa file. Selama Plan, tidak ada file proyek berubah dan tidak ada shell dijalankan.

Pengguna berpindah Build dan memberi instruksi menerapkan rencana. Agent memakai patch; satu shell command ditolak untuk membuktikan kontrol, kemudian command validasi yang diperlukan disetujui. Hasil akhir menyebut perubahan dan hasil aktual. `/diff` tidak memasukkan perubahan pengguna sebelumnya. Tutup/resume untuk memastikan rencana serta status pulih tanpa mengulang tools.

### FLOW-02 Proyek baru

Mulai pada direktori kosong yang sudah ada. Minta aplikasi kecil yang dapat dijalankan dan mempunyai pemeriksaan deterministik sederhana. Plan merinci struktur dan validasi; Build membuat file, meminta izin untuk scaffolding/install/run yang dibutuhkan, dan melaporkan hasil aktual.

Gunakan dependensi minimal atau fixture lokal pada acceptance deterministik. Pengujian instalasi network nyata dilakukan terpisah; network unavailable harus memberikan error yang dapat ditindaklanjuti. Folder dependensi tidak dimasukkan seluruhnya ke konteks atau snapshot review.

Proyek tidak wajib memiliki Git. File baru yang dicakup review terlihat sebagai tambahan; keluaran shell dan status validasi disimpan; session dapat dilanjutkan. Keberhasilan tidak bergantung pada nama model tertentu, tetapi provider live yang dipakai harus dicatat.

## Pengukuran resource

| ID | Metrik | Metode dan target |
| --- | --- | --- |
| PERF-01 | Kesiapan prompt | Dari proses diluncurkan hingga input siap; 20 run warm-cache, p95 ≤500 ms. Config/session lokal kecil; model discovery tidak memblokir. |
| PERF-02 | RAM idle | Maksimum RSS yang diamati selama 10 detik setelah session baru idle, tanpa request atau child process; ≤100 MB. |
| PERF-03 | Binary rilis | Ukuran executable setelah release build dan strip; ≤40 MB desimal, per target platform. |
| PERF-04 | Beban sesi | Catat peak RSS selama streaming, pencarian, resume besar, snapshot, dan shell. Pisahkan RSS parent dari child; tidak mengklaim target idle sebagai peak target. |

Laporan mencatat hardware/CPU, arsitektur, OS, Rust/toolchain, dependency lock, build profile, jumlah sampel, versi aplikasi, ukuran repo/session, dan emulator terminal. Pengukuran cold-cache, startup pertama kali, OAuth, dan latensi LLM dilaporkan terpisah bila dilakukan. MB berarti 1.000.000 byte; MiB dalam batas tools berarti 1.048.576 byte.

Target harus dibuktikan pada baseline macOS dan Linux yang dicatat. Sampel RAM nca bukan baseline ruvx dan tidak dipakai sebagai bukti komparatif performa.

## Gerbang penerimaan

Rilis v1 memerlukan seluruh REQ memiliki bukti lulus; tidak ada pelanggaran mode atau tool yang berjalan tanpa approval; dua workflow berhasil; terminal pulih pada seluruh jalur exit; serta ketiga jalur provider terbukti sesuai scope. Keterbatasan akun/live environment ditulis sebagai pengujian belum selesai, bukan lulus berbasis mock.

Pada tahap dokumentasi awal, verifikasi terbatas pada struktur, keputusan, tautan, dan sumber. Hasil pengujian aplikasi setelah implementasi dicatat terpisah; matriks ini tidak otomatis memberi status lulus kepada seluruh skenario hanya karena test suite lokal berhasil.
