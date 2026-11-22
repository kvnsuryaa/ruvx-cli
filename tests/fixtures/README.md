# OAuth test fixtures

The RSA key in this directory is a generated, public test fixture. It is intentionally committed for deterministic signature-validation tests and must never be used for real accounts or credentials.

# MCP stdio

`mcp-stdio.sh` adalah fixture JSON-RPC lokal yang dijalankan dengan `/bin/sh` dalam workspace sementara. Fixture menyediakan handshake, discovery dua halaman, ping, request sampling yang harus ditolak, dan tool echo. `server.pid` serta `server.requests` dipakai untuk memeriksa cleanup dan pesan client; tidak membutuhkan package manager, jaringan, atau provider eksternal.
