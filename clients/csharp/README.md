# C# client seed - `dd-rust-network-mutex`

Dependency-free C# protocol mirror for the broker wire format. It uses enums
for request/response discriminators, JSONL request builders, and a small
`System.Text.Json` response parser for the lock/composite fields clients need.

## Run

```bash
dotnet run --project clients/csharp
```

The test is offline: no broker is required.

