# Konteks dan Sesi ruvx v1

ruvx memiliki riwayat lokal sendiri. Konteks yang dikirim ke model adalah tampilan terpilih dari riwayat tersebut, ditambah instruksi dan data proyek yang relevan. Riwayat asli tetap tersedia ketika pengguna melakukan compaction.

## Konteks sesuai kebutuhan

Pada startup, ruvx memuat metadata workspace dan instruksi root yang kecil; ia tidak membaca seluruh isi repo atau membangun embedding index. Agent memakai list/search/read untuk menemukan file yang dibutuhkan. Referensi `@path` dari pengguna menjadi permintaan membaca file eksplisit melalui batas tools yang sama.

| Bagian konteks | Asal | Aturan |
| --- | --- | --- |
| Instruksi core dan mode | Aplikasi | Kebijakan tools dan mode aktif; model tidak boleh mengubahnya. |
| Permintaan pengguna | Pesan session | Instruksi tugas serta perubahan arah terbaru. |
| Instruksi proyek | AGENTS.md dalam workspace | Berlaku sesuai direktori target; bukan otorisasi shell atau perubahan mode. |
| Ringkasan aktif | Hasil `/compact` yang berhasil | Menggantikan prefix konteks berikutnya, tidak menghapus jurnal asli. |
| Percakapan setelah ringkasan | Jurnal session | Urutan role dan pasangan tool harus konsisten. |
| File dan tool results | Referensi eksplisit dan tools | Data diberi path, versi, serta penanda potongan. |
| Artefak khusus adapter | Respons provider | Dipertahankan pada route yang membutuhkannya; tidak dipaksakan lintas provider. |

ruvx membedakan instruksi dari isi file dan hasil shell. Data proyek tidak dapat menambah izin, mengubah mode, atau mengubah profil tanpa tindakan pengguna.

## Resolusi AGENTS.md

Default v1 membaca AGENTS.md pada root workspace dan, ketika suatu direktori dieksplorasi atau file hendak disentuh, instruksi yang berada pada jalur root menuju direktori tersebut. File yang lebih dekat memberi instruksi khusus untuk subtree-nya.

Instruksi di luar root tidak dicari otomatis. ruvx tidak menganggap seluruh AGENTS.md dalam repo berlaku untuk semua file. Jika dua instruksi proyek bertentangan, aturan terdekat pada target berlaku selama konsisten dengan permintaan pengguna dan kebijakan aplikasi.

Simpan path dan hash instruksi yang digunakan. Saat resume atau sebelum request setelah file instruksi berubah, muat ulang versi disk dan tandai perubahan konteks. Instruksi file tetap tidak dapat mengizinkan shell di Plan atau meniadakan approval Build.

File instruksi melebihi batas teks memberi error yang dapat diperbaiki sebelum dipakai; jangan memotong aturan proyek tanpa pemberitahuan.

## Siklus hidup session

Session terkait satu workspace kanonis dan memiliki ID lokal. Metadata minimum meliputi versi format, waktu dibuat/diperbarui, mode, profil/model terpilih, serta status run terakhir.

Pesan pengguna disimpan sebelum request. Respons yang selesai, tool intents, tool results, perubahan mode/model, approval, compaction, dan alasan run berhenti dicatat berurutan. Keluaran panjang disimpan sebagai artefak terpisah agar jurnal dan RAM tetap terkendali.

Delta token bukan unit durability utama. Ketika stream terputus normal atau dibatalkan, simpan teks parsial dan status terputus bila masih tersedia. Crash mendadak dapat kehilangan tail streaming yang belum difinalkan; itu tidak mengubah tindakan tools yang sudah dicatat.

Session baru selalu Plan. Resume memulihkan mode yang tersimpan dan menunjukkannya jelas; tidak memulai request, tools, shell, atau perpindahan Build otomatis.

Model session yang tersimpan memiliki prioritas atas default global. Jika session belum memiliki model, startup atau perpindahan session menerapkan pilihan terakhir pengguna yang masih valid dan mencatatnya ke jurnal. `/new` membaca ulang konfigurasi dan default global, mempertahankan draft, serta tetap dimulai dalam Plan. Pemulihan otomatis tidak menulis ulang default global dan tidak memulai inference. Model session lama yang sudah tidak tersedia tetap ditampilkan dengan pemberitahuan untuk memperbaiki koneksi atau memilih model; tidak diganti otomatis.

## Resume dan recovery

1. Baca header/version dan periksa workspace yang tercatat.
2. Ambil lock session/workspace. Proses lain tidak boleh menjadi writer untuk proyek yang sama.
3. Replay jurnal menjadi state aplikasi, bukan menjadi eksekusi tools.
4. Validasi artefak dan rekonsiliasi run yang belum ditutup.
5. Tampilkan ringkasan session, mode, model, perubahan, dan status tindakan terakhir.
6. Tunggu input pengguna. Jika model atau login tidak siap, riwayat tetap dapat dilihat dan koneksi dapat diperbaiki.

| Kondisi saat pemulihan | Perilaku |
| --- | --- |
| Request mulai tanpa completion | Tandai interrupted; jangan lanjut network otomatis. |
| Tool intent tanpa hasil final | Tandai execution uncertain; jangan mengeksekusi ulang. |
| Shell pernah mulai tanpa hasil | Minta pengguna memeriksa proses/efek; tidak mengklaim command pasti gagal atau berhasil. |
| Patch intent belum final | Periksa hash file dibanding baseline/hasil yang tercatat; jika tidak cukup bukti, tandai uncertain. |
| Workspace tidak ada atau pindah | Tampilkan masalah; v1 tidak memindahkan root session secara implisit. |
| Login kedaluwarsa | Buka riwayat; generasi menunggu koneksi siap. |
| Versi jurnal tidak dikenal | Buka pesan kompatibilitas; jangan menulis ulang format yang tidak dipahami. |

Jika satu baris terakhir JSONL terpotong akibat crash, pertahankan berkas asli dan ambil prefix yang valid ke salinan recovery sebelum melanjutkan penulisan. Kerusakan di tengah jurnal menghentikan resume writable. Tidak ada penghilangan baris korup secara diam-diam.

## Kapasitas konteks

Sebelum request, context manager menghitung estimasi ukuran input beserta tool definitions dan cadangan respons. Metadata limit dari model atau konfigurasi digunakan bila tersedia. Estimasi ditandai sebagai estimasi, bukan angka usage provider.

Jika estimasi tidak lagi muat pada limit yang diketahui, pause sebelum request. Jika limit tidak diketahui, jangan mengarang nilainya; gunakan konfigurasi pengguna dan tangani penolakan kapasitas dari provider dengan alasan yang jelas. Dalam kedua keadaan, tawarkan `/compact` atau `/new`.

ruvx tidak melakukan sliding-window deletion, ringkasan otomatis, atau pemotongan hasil lama sebagai pengganti keputusan pengguna. Truncation per tool yang terlihat sejak hasilnya dibuat merupakan kontrak tool, bukan penghapusan history secara diam-diam.

## Compaction manual

`/compact` hanya dapat dimulai ketika tidak ada run aktif atau tindakan yang belum terselesaikan. UI menunjukkan bahwa operasi ini memanggil model dan cakupan riwayat yang akan diringkas. Pengguna dapat membatalkannya.

Default teknis mempertahankan dua percakapan pengguna terbaru beserta seluruh rangkaian tool call/result yang terkait. Prefix sebelumnya diringkas pada batas percakapan yang utuh. Jika prefix terlalu kecil, beri tahu bahwa compaction belum bermanfaat; jangan membuat ringkasan kosong.

Ringkasan harus memuat tujuan pengguna, keputusan yang sudah dibuat, rencana, perubahan penting, hasil validasi yang diketahui, kegagalan, pekerjaan tersisa, serta rujukan file/artefak yang relevan. Instruksi core dan instruksi proyek aktif dimasukkan kembali secara terpisah, bukan bergantung pada akurasi ringkasan.

Ringkasan baru menjadi konteks aktif setelah request berhasil dan hasil tersimpan. Cancel/error mempertahankan konteks sebelumnya. Operasi ini sendiri harus muat dalam limit model. Jika tidak muat, tampilkan alasan dan tawarkan session baru; v1 tidak meluncurkan rantai ringkasan tambahan otomatis.

Pada compaction manual yang berhasil, konteks percakapan terbaru yang dipertahankan juga dinormalisasi ke teks dan pasangan tool call/result umum. Metadata reasoning opaque milik provider dikeluarkan dari proyeksi aktif agar pemilihan model/provider lain dapat dilakukan tanpa membawa item yang tidak dapat diterjemahkan. Artefak asli tetap utuh di disk. Normalisasi ini hanya terjadi melalui compaction eksplisit, bukan saat memilih model secara diam-diam.

Jurnal mencatat versi ringkasan, rentang event sumber, model/profil pembuatnya, serta event terakhir yang dicakup. Riwayat lengkap dan baseline perubahan tidak dihapus. Compaction bukan reset budget run dan bukan approval untuk menerapkan rencana.

## Pergantian model atau profil

Perubahan dilakukan ketika idle dan dicatat sebelum request berikutnya. Data umum percakapan dan tool results dapat diterjemahkan lewat adapter. Opaque provider-specific context yang tidak dapat dibawa membuat pergantian tertahan dengan pilihan compaction manual atau session baru.

Persetujuan shell, state autentikasi baru, atau perubahan model tidak menimbulkan pengulangan tools. Usage dan label akun ditampilkan menurut profil aktif, sementara jurnal tetap mencatat profil pada setiap turn.

## Review diff dan perubahan eksternal

Baseline per file diambil saat edit pertama oleh session, bukan dari Git HEAD. Karena itu, repo tanpa Git tetap dapat direview dan perubahan pengguna yang sudah ada menjadi bagian baseline.

Versi setelah edit terakhir dicatat. Jika versi disk berbeda saat review atau resume, tampilkan bahwa file berubah di luar jejak session. Diff yang mengklaim hanya perubahan agent memakai baseline dan versi hasil session yang tersimpan; keadaan disk terkini ditandai terpisah.

Shell dapat mengubah file tanpa melewati patch tool. Karena itu, ambil snapshot teks sebelum approval dan sesudah command sesuai [cakupan review shell](05-agent-tools-and-controls.md#baseline-perubahan-shell). Baseline awal session dipertahankan untuk file yang pernah diedit; file lain memakai snapshot sebelum command ketika pertama kali berubah. File baru menjadi tambahan.

UI membedakan diff patch yang teratribusi dan perubahan yang teramati selama interval shell. Ignored files, dependency/build directories, binary, file terlalu besar, serta lokasi di luar workspace tidak dijanjikan tercakup diff. Perubahan editor lain dalam interval itu juga tidak dapat diatribusikan pasti. Cakupan dan keterbatasan ditampilkan, sehingga pre-existing dirty state tetap menjadi baseline pada file yang dicakup tanpa mengklaim semua efek shell dapat direkonstruksi.

## Retensi dan akses

V1 tidak menghapus session otomatis dan tidak menyinkronkan ke cloud. Pengguna dapat menghapus data lokal secara manual setelah menutup session. File session dapat berisi kode dan keluaran tools; permission penyimpanan mengikuti [konfigurasi dan storage](07-configuration-and-storage.md). Kredensial koneksi disimpan terpisah dan tidak menjadi bagian export atau ringkasan model.
