using NetworkMutex;

static void Check(bool condition, string name)
{
    if (condition)
    {
        return;
    }

    Console.Error.WriteLine($"FAIL: {name}");
    Environment.Exit(1);
}

var single = Protocol.LockRequestSingle("u-1", "k1", ttlMs: 4000, maxHolders: 1, wait: false);
Check(single.Contains("\"type\":\"lock\""), "lock type");
Check(single.Contains("\"key\":\"k1\""), "key field");
Check(single.Contains("\"ttl\":4000"), "ttl field");
Check(single.Contains("\"max\":1"), "max field");
Check(single.Contains("\"wait\":false"), "wait false");
Check(single.EndsWith('\n'), "newline");

var composite = Protocol.LockRequestComposite("u-2", ["c", "a", "b"], wait: true);
Check(composite.Contains("\"keys\":[\"c\",\"a\",\"b\"]"), "composite keys");
Check(composite.Contains("\"wait\":true"), "wait true");

var oversizeRejected = false;
try
{
    _ = Protocol.LockRequestComposite("u", ["a", "b", "c", "d", "e", "f"]);
}
catch (ArgumentException)
{
    oversizeRejected = true;
}
Check(oversizeRejected, "oversize composite rejected");

var response = Protocol.DecodeResponse(
    "{\"type\":\"compositeLock\",\"uuid\":\"u\",\"keys\":[\"a\",\"b\"],\"acquired\":true,\"lockUuid\":\"L\",\"fencingTokens\":{\"a\":1780240060223,\"b\":12}}");
Check(response.Type == ResponseType.CompositeLock, "composite response type");
Check(response.Acquired == true, "composite acquired");
Check(response.LockUuid == "L", "lock uuid");
Check(response.FencingTokens["a"] == 1780240060223UL, "64-bit token");
Check(Protocol.ResponseTypeFromWire("totallyBogus") == ResponseType.Unknown, "unknown response");

Console.WriteLine("[test-csharp] all protocol tests passed");

