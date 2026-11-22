# Local protocol fixture. Invoked with /bin/sh; no packages or network needed.
printf '%s' "$$" > server.pid
n=0
while IFS= read -r line; do
    printf '%s\n' "$line" >> server.requests
    case "$line" in
        *'"method":"initialize"'*)
            n=$((n+1))
            printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":"2024-11-05","capabilities":{"tools":{}},"serverInfo":{"name":"fixture","version":"1"}}}\n' "$n";;
        *'"method":"tools/list"'*)
            n=$((n+1))
            printf '%s\n' '{"jsonrpc":"2.0","method":"notifications/tools/list_changed"}'
            printf '%s\n' '{"jsonrpc":"2.0","id":"ping","method":"ping"}'
            printf '%s\n' '{"jsonrpc":"2.0","id":"sampling","method":"sampling/createMessage","params":{}}'
            case "$line" in
                *'"cursor":"page2"'*)
                    printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"echo","description":"Echo input","inputSchema":{"type":"object","properties":{"text":{"type":"string"}},"required":["text"]}}]}}\n' "$n";;
                *)
                    printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"status","inputSchema":{"type":"object"}}],"nextCursor":"page2"}}\n' "$n";;
            esac;;
        *'"method":"tools/call"'*)
            n=$((n+1))
            printf '{"jsonrpc":"2.0","id":%s,"result":{"content":[{"type":"text","text":"fixture result"}]}}\n' "$n";;
    esac
done
