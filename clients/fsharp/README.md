# F# client seed - `dd-rust-network-mutex`

Dependency-free F# protocol mirror for the broker wire format. It uses
discriminated unions for request/response type tags, JSONL request builders,
and a small `System.Text.Json` response parser.

## Run

```bash
dotnet run --project clients/fsharp
```

The test is offline: no broker is required.

