# Provider dan Autentikasi ruvx v1

ruvx memisahkan cara memperoleh kredensial dari cara mengirim request LLM. Satu provider dapat mendukung beberapa metode login, tetapi setiap pasangan provider, model, dan metode auth tetap memiliki kapabilitas sendiri.

Client baru memakai identitas `ruvx` dan namespace tools ChatGPT `ruvx`. Untuk riwayat opaque lama yang masih membawa namespace `kovr`, request mempertahankan namespace lama itu tanpa menulis ulang item provider. Parser menerima prefix tools kedua nama untuk kompatibilitas sesi; sesi baru memakai namespace `ruvx`.

## Matriks koneksi v1

| Profil | Metode auth | Protokol inferensi | Catatan |
| --- | --- | --- | --- |
| OpenAI API | API key tersimpan atau environment variable | Responses melalui HTTPS/SSE | Pemakaian dan tagihan mengikuti akun API. |
| ChatGPT | Sign in with ChatGPT untuk penggunaan paket yang memenuhi syarat | Responses melalui HTTPS/SSE | Scope inference, kelayakan akun, daftar model, dan batas preview harus dipenuhi. |
| Custom compatible | API key tersimpan, environment variable, atau none yang dipilih eksplisit | Chat Completions melalui HTTP/SSE | Endpoint harus mendukung tools dan streaming yang dibutuhkan agent. |

Login ChatGPT dan OpenAI API key boleh berada di profil berbeda. ruvx tidak menyalin kredensial dari instalasi Codex atau OpenCode dan tidak memerlukan kedua aplikasi tersebut sebagai runtime.

OpenCode sendiri menyediakan pilihan login ChatGPT dan API key; custom provider memiliki endpoint serta konfigurasi model tersendiri. Pola pilihan koneksi itu menjadi rujukan UX ruvx. Implementasi provider ruvx tetap milik core sendiri. [OpenCode providers](https://opencode.ai/docs/providers/)

Langganan suatu layanan tidak otomatis memberikan akses OAuth atau API bagi aplikasi pihak ketiga. Dukungan login di ruvx hanya ditambahkan bila jalur layanan terkait sudah diverifikasi. Claude subscription, Copilot, serta provider native tambahan berada di luar v1.

## Antarmuka auth adapter

| Operasi konseptual | Kontrak |
| --- | --- |
| Daftar metode | Mengembalikan metode yang benar-benar didukung profil, bukan menu OAuth universal. |
| Mulai koneksi | Membuat attempt terpisah; tidak mengganti koneksi aktif sampai validasi selesai. |
| Selesaikan koneksi | Memvalidasi kredensial, identitas dan izin yang relevan sebelum menyimpan status siap. |
| Resolve credentials | Mengambil key atau token terbaru melalui jalur privat, tanpa memasukkannya ke transcript. |
| Refresh | Memperbarui token dengan lock lintas proses untuk identitas/registrasi yang sama. |
| Status | Mengembalikan label akun aman, metode, kesiapan inference, dan alasan tidak siap. |
| Disconnect | Menghapus kredensial lokal profil yang dipilih; session tetap tersedia. |

API key dan bearer access token tidak diperlakukan sebagai nilai konfigurasi yang dapat dicetak bebas. Seluruh header auth dan token endpoint response dikecualikan dari log umum.

## Login ChatGPT

Jalur yang dipilih adalah direct Sign in with ChatGPT untuk aplikasi lokal yang memenuhi ketentuan akses paket. Dokumentasi OpenAI menyebut aplikasi open-source/lokal dan menjelaskan akses paket yang memenuhi syarat. Kelayakan akun perlu diuji pada akun pengguna; keberhasilan login identitas saja belum membuktikan akses inference. [Overview](https://developers.openai.com/siwc/token-sharing-open-source), [cookbook resmi](https://developers.openai.com/cookbook/articles/sign-in-with-chatgpt)

### Alur registrasi dan sign-in

1. Siapkan host ID stabil, callback loopback `127.0.0.1`, serta state, nonce, dan PKCE S256 baru untuk setiap attempt.
2. Registrasi pertama memakai entrypoint `dynamic_agent_client` dan nama aplikasi sebenarnya. Simpan issued client ID yang dikembalikan, bukan entrypoint tersebut.
3. Buka browser sistem. Kembalinya callback harus cocok dengan attempt aktif dan URI redirect yang digunakan saat pertukaran code.
4. Tukarkan code sebagai public client, lalu validasi ID token dan izin inference sebelum mengaktifkan koneksi.
5. Simpan kredensial per identitas terverifikasi dan issued client ID secara atomik dengan akses pemilik saja. Reauthorization menggunakan registrasi tersimpan.

Port loopback boleh berubah sesuai dokumentasi, tetapi host dan path callback harus konsisten. Jangan mengganti `127.0.0.1` dengan `localhost`. Validasi meliputi signature, issuer, audience, expiry, nonce, state, dan scope akses paket. Detail parameter dan endpoint harus mengikuti dokumentasi resmi saat implementasi. [Registration and sign-in](https://developers.openai.com/siwc/token-sharing-open-source/sign-in)

Refresh yang merotasi token harus diserialkan. Simpan access token, expiry, scope, serta refresh token baru sebagai satu perubahan atomik. Identitas akun dan registrasi tidak boleh tertukar saat pengguna menambah koneksi. [Accounts and sessions](https://developers.openai.com/siwc/token-sharing-open-source/profiles-and-sessions)

### Capability khusus akses paket

Direct inference memakai API publik `/v1/models` dan `/v1/responses`. Daftar model mengikuti akses akun; endpoint ChatGPT internal tidak menjadi jalur integrasi ruvx. [Models and inference](https://developers.openai.com/siwc/token-sharing-open-source/models-and-inference)

Adapter paket membutuhkan streaming dan penyimpanan percakapan di sisi client: request menggunakan `stream: true`, `store: false`, serta history yang disediakan ruvx. Beberapa parameter API biasa, termasuk `temperature` dan `max_output_tokens`, tidak diterima pada jalur preview. Function/custom tools harus mengikuti bentuk namespace yang didukung; hosted MCP dan beberapa hosted tools tidak tersedia. [Preview limitations](https://developers.openai.com/siwc/token-sharing-open-source/preview-limitations)

Konsekuensi desain: adapter tidak boleh sekadar mengganti API key dengan token pada payload API biasa. Ia membangun payload sesuai capability auth yang aktif. Core tetap memakai tools lokal; adapter menerjemahkan nama tools dan envelope ke format yang diterima route.

## Provider adapter

| Operasi | Masukan dan keluaran |
| --- | --- |
| Validate selection | Profil, model, auth mode, dan kebutuhan tools → siap atau alasan kegagalan. |
| Discover models | Daftar akun/provider bila tersedia; custom dapat memakai daftar eksplisit dari config. |
| Build request | Konteks aktif, tools mode, pilihan model, capability → request protokol. |
| Stream turn | Request → delta teks, tool calls, completion, usage, atau error terstruktur. |
| Cancel | Menutup request aktif tanpa menjalankan tool yang belum tervalidasi. |

Capability minimum mencatat protokol, tool calling, streaming, model ID aktual, context limit bila diketahui, kemampuan input, dan parameter yang didukung. Context limit yang belum diketahui tidak diganti dengan angka besar yang diasumsikan benar.

Model dipilih pengguna dan dicatat pada session. Tidak ada nama model terbaru yang di-hardcode dalam rancangan. Pemilihan custom model dengan dukungan tool yang tidak diketahui memerlukan konfigurasi capability eksplisit sebelum dipakai sebagai agent.

## Responses dan Chat Completions

| Aspek | Responses | Chat Completions |
| --- | --- | --- |
| Konteks | Input items dan instructions/developer sesuai route | Messages, assistant tool_calls, dan tool-result messages |
| Identitas tool stream | Output item/index serta call identity | Tool-call index dan ID dalam delta |
| Argumen stream | Fragmen argumen pada event terkait | Fragmen dapat datang tanpa ID/name berulang |
| Hasil tool | Item hasil sesuai call identity | Pesan tool dengan tool_call_id |
| State lokal | ruvx memiliki jurnal dan konteks aktif | ruvx memiliki jurnal dan konteks aktif |

Kedua protokol tidak disamakan pada level JSON wire. Adapter menyatukannya menjadi representasi core yang menjaga urutan dan pasangan tool call/result. Fragmen argumen baru boleh menjadi tindakan setelah respons final berhasil, JSON lengkap, schema valid, dan dispatcher menyetujuinya. [Function calling dan streaming](https://developers.openai.com/api/docs/guides/function-calling#streaming)

Reasoning item atau metadata opaque yang diperlukan provider dipertahankan sebagai artefak terikat adapter. Jika tidak dapat diterjemahkan saat berpindah provider, hentikan perpindahan dan tawarkan `/compact` atau session baru; jangan menghapusnya diam-diam.

## Endpoint custom

Base URL konfigurasi adalah API prefix lengkap, misalnya berakhir `/v1`; adapter menambahkan resource Chat Completions tanpa menggandakan prefix. HTTPS digunakan untuk layanan remote; HTTP hanya diperbolehkan untuk endpoint loopback lokal pada v1.

Profil custom menetapkan model ID dan capability. Endpoint tanpa route daftar model tetap dapat digunakan melalui model eksplisit. Discovery tidak otomatis melakukan paid generation probe. Respons yang tidak mendukung tools atau streaming menghasilkan error ketidakcocokan, bukan fallback diam-diam ke perilaku chat biasa.

## Error, retry, dan pelaporan penggunaan

| Kejadian | Perilaku ruvx |
| --- | --- |
| Consent dibatalkan | Tutup attempt, pertahankan profil aktif sebelumnya. |
| OAuth identity valid tetapi scope inference tidak ada | Status koneksi belum siap untuk agent; berikan jalur reauthorization. |
| Token perlu refresh | Refresh melalui adapter; bila ditolak, pause dan minta login ulang. |
| API key ditolak | Pause; tunjukkan profil yang perlu diperbaiki tanpa mencetak key. |
| Rate limit atau kuota paket | Pause dengan informasi provider yang aman; tidak memakai profil lain otomatis. |
| Overload atau server sementara tidak tersedia sebelum keluaran | Retry otomatis terbatas dengan jeda bertahap; status terlihat dan Ctrl+C membatalkan. |
| Timeout atau koneksi sementara gagal sebelum header respons | Retry otomatis dengan batas dan jeda yang sama; pesan mengarahkan ke pemeriksaan koneksi jaringan. |
| Stream putus | Simpan status parsial, jangan jalankan calls belum final, tunggu tindakan pengguna. |
| Model/parameter tidak didukung | Tampilkan capability yang tidak cocok; jangan mengubah payload ke semantik berbeda diam-diam. |
| Discovery gagal | Pertahankan inventory terakhir yang terkait profil tersebut atau daftar custom eksplisit; tandai statusnya. |

Request generasi yang ditolak sementara sebelum keluaran diulang otomatis maksimal **3 kali**, dengan jeda default **2, 4, lalu 8 detik**. Ini berlaku untuk HTTP 500/502/503/504 serta kode overload/server unavailable yang dikenali, termasuk `server_is_overloaded` dan `service_unavailable_error`. Timeout saat mengirim request atau menunggu header, serta kegagalan koneksi sementara yang dikenali (misalnya connection refused/reset), juga mendapat retry sebelum respons diterima. Retry menggunakan request identik pada turn yang sama, tidak mengganti model atau provider, dan tidak mengulang tool yang sudah selesai. Status retry muncul di chat sementara indikator aktivitas tetap berjalan; Ctrl+C membatalkan waktu tunggu dan mencegah request berikutnya.

`network.max_retries` mengatur batas 0–10 (0 menonaktifkan retry), dan `network.retry_initial_delay_seconds` mengatur jeda awal 1–60 detik. Jeda berlipat dua dengan batas 60 detik. Header `Retry-After` numerik dihormati bila lebih panjang; nilai di atas 60 detik atau format yang tidak didukung menghentikan retry otomatis agar tidak mengirim terlalu awal. Autentikasi, kuota/rate limit, model/parameter, kapasitas konteks, kesalahan sertifikat TLS, kegagalan transport yang tidak dikenal, dan error lain yang tidak dikenali memerlukan tindakan pengguna. Kegagalan transport setelah header respons diterima tidak diulang otomatis; output parsial dipertahankan. Setelah teks, reasoning, tool call, atau usage mulai diterima, kegagalan dipertahankan tanpa retry otomatis. Refresh kredensial sebelum request bukan pengulangan tool atau generasi. Tidak ada retry otomatis shell atau patch.

Error terstruktur dalam stream menampilkan identifier `code`, `type`, atau alasan incomplete yang tersedia, disertai tindakan lanjut sesuai kategori. Contoh: `usage_limit_reached` mengarahkan pengguna memeriksa batas pemakaian akun, `context_length_exceeded` ke `/compact` atau `/new`, dan `invalid_api_key` ke `/connect`. Hanya identifier pendek berisi huruf kecil ASCII, angka, dan underscore yang ditampilkan; pesan dan body mentah provider tidak dicetak karena dapat memuat kredensial atau isi input. Jika identifier tidak tersedia, penyebab ditandai `unspecified`. Detail aman kegagalan akhir tersimpan dalam jurnal sesi. Teks parsial dipertahankan; tool call yang belum final tidak dieksekusi.

Usage token ditampilkan bila provider menyediakannya. Nilai yang hilang ditandai tidak tersedia; pemakaian paket tidak diberi biaya uang nol sebagai asumsi universal. Perkiraan konteks dibedakan dari usage yang dilaporkan provider.

## Kriteria penerimaan

Login ChatGPT, API key OpenAI, dan custom compatible harus diuji terpisah. Mock menguji protokol dan error; pengujian akun nyata yang memenuhi syarat dibutuhkan untuk membuktikan akses subscription. Versi docs dan model yang dipakai dicatat, karena kondisi preview dapat berubah. Lihat [test matrix](08-testing-and-acceptance.md).

## Reasoning effort

Pilihan reasoning effort tersimpan pada model. Responses memakai `reasoning.effort`; gateway Chat Completions memakai `reasoning_effort`. Default tidak menambahkan field. Unknown capability harus ditetapkan eksplisit atau dibaca dari metadata; tidak ada fallback level diam-diam. Kontrak dan sumber tersedia pada [effort per model](../model-efforts.md).
