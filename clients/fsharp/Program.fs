open System
open NetworkMutex

let check condition name =
  if not condition then
    eprintfn $"FAIL: {name}"
    Environment.Exit 1

let single = Protocol.lockRequestSingle "u-1" "k1" 4000 (Some 1) (Some false)
check (single.Contains("\"type\":\"lock\"")) "lock type"
check (single.Contains("\"key\":\"k1\"")) "key field"
check (single.Contains("\"ttl\":4000")) "ttl field"
check (single.Contains("\"max\":1")) "max field"
check (single.Contains("\"wait\":false")) "wait false"
check (single.EndsWith "\n") "newline"

let composite = Protocol.lockRequestComposite "u-2" [ "c"; "a"; "b" ] 0 (Some true)
check (composite.Contains("\"keys\":[\"c\",\"a\",\"b\"]")) "composite keys"
check (composite.Contains("\"wait\":true")) "wait true"

let oversizeRejected =
  try
    Protocol.lockRequestComposite "u" [ "a"; "b"; "c"; "d"; "e"; "f" ] 0 None |> ignore
    false
  with :? ArgumentException -> true

check oversizeRejected "oversize composite rejected"

let response =
  Protocol.decodeResponse
    "{\"type\":\"compositeLock\",\"uuid\":\"u\",\"keys\":[\"a\",\"b\"],\"acquired\":true,\"lockUuid\":\"L\",\"fencingTokens\":{\"a\":1780240060223,\"b\":12}}"

check (response.Type = ResponseType.CompositeLock) "composite response type"
check (response.Acquired = Some true) "composite acquired"
check (response.LockUuid = Some "L") "lock uuid"
check (response.FencingTokens["a"] = 1780240060223UL) "64-bit token"
check (Protocol.responseTypeFromWire "totallyBogus" = ResponseType.Unknown) "unknown response"

printfn "[test-fsharp] all protocol tests passed"

