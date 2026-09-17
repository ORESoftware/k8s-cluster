using System.Text.Json;

namespace NetworkMutex;

public enum RequestType
{
    Version,
    Auth,
    Lock,
    Unlock,
    RegisterRead,
    RegisterWrite,
    EndRead,
    EndWrite,
    LockInfo,
    Ls,
    Heartbeat,
}

public enum ResponseType
{
    Version,
    Auth,
    Lock,
    CompositeLock,
    Unlock,
    RegisterReadResult,
    RegisterWriteResult,
    EndReadResult,
    EndWriteResult,
    LockInfo,
    LsResult,
    Reelection,
    Error,
    Ok,
    Unknown,
}

public sealed record Response(
    ResponseType Type,
    string Uuid,
    string? Key,
    IReadOnlyList<string> Keys,
    bool? Acquired,
    bool? Unlocked,
    string? LockUuid,
    ulong? FencingToken,
    IReadOnlyDictionary<string, ulong> FencingTokens,
    string? Error);

public static class Protocol
{
    public const string ProtocolVersion = "0.1.0";
    public const int MaxCompositeKeys = 5;

    public static readonly string[] RequestTypes =
    [
        "version",
        "auth",
        "lock",
        "unlock",
        "registerRead",
        "registerWrite",
        "endRead",
        "endWrite",
        "lockInfo",
        "ls",
        "heartbeat",
    ];

    public static readonly string[] ResponseTypes =
    [
        "version",
        "auth",
        "lock",
        "compositeLock",
        "unlock",
        "registerReadResult",
        "registerWriteResult",
        "endReadResult",
        "endWriteResult",
        "lockInfo",
        "lsResult",
        "reelection",
        "error",
        "ok",
    ];

    public static string ToWire(RequestType type) => type switch
    {
        RequestType.Version => "version",
        RequestType.Auth => "auth",
        RequestType.Lock => "lock",
        RequestType.Unlock => "unlock",
        RequestType.RegisterRead => "registerRead",
        RequestType.RegisterWrite => "registerWrite",
        RequestType.EndRead => "endRead",
        RequestType.EndWrite => "endWrite",
        RequestType.LockInfo => "lockInfo",
        RequestType.Ls => "ls",
        RequestType.Heartbeat => "heartbeat",
        _ => throw new ArgumentOutOfRangeException(nameof(type), type, null),
    };

    public static ResponseType ResponseTypeFromWire(string value) => value switch
    {
        "version" => ResponseType.Version,
        "auth" => ResponseType.Auth,
        "lock" => ResponseType.Lock,
        "compositeLock" => ResponseType.CompositeLock,
        "unlock" => ResponseType.Unlock,
        "registerReadResult" => ResponseType.RegisterReadResult,
        "registerWriteResult" => ResponseType.RegisterWriteResult,
        "endReadResult" => ResponseType.EndReadResult,
        "endWriteResult" => ResponseType.EndWriteResult,
        "lockInfo" => ResponseType.LockInfo,
        "lsResult" => ResponseType.LsResult,
        "reelection" => ResponseType.Reelection,
        "error" => ResponseType.Error,
        "ok" => ResponseType.Ok,
        _ => ResponseType.Unknown,
    };

    public static string VersionRequest(string uuid, string value = ProtocolVersion) =>
        Frame(new Dictionary<string, object?> { ["type"] = "version", ["uuid"] = uuid, ["value"] = value });

    public static string AuthRequest(string uuid, string token) =>
        Frame(new Dictionary<string, object?> { ["type"] = "auth", ["uuid"] = uuid, ["token"] = token });

    public static string LockRequestSingle(
        string uuid,
        string key,
        long ttlMs = 0,
        int? maxHolders = null,
        bool? wait = null) =>
        Frame(new Dictionary<string, object?>
        {
            ["type"] = "lock",
            ["uuid"] = uuid,
            ["key"] = key,
            ["ttl"] = ttlMs > 0 ? ttlMs : null,
            ["max"] = maxHolders,
            ["wait"] = wait,
        });

    public static string LockRequestComposite(string uuid, IReadOnlyList<string> keys, long ttlMs = 0, bool? wait = null)
    {
        if (keys.Count is < 1 or > MaxCompositeKeys)
        {
            throw new ArgumentException($"composite key count must be 1..=5, got {keys.Count}", nameof(keys));
        }

        return Frame(new Dictionary<string, object?>
        {
            ["type"] = "lock",
            ["uuid"] = uuid,
            ["keys"] = keys,
            ["ttl"] = ttlMs > 0 ? ttlMs : null,
            ["wait"] = wait,
        });
    }

    public static string UnlockRequestSingle(string uuid, string key, string? lockUuid, bool force = false) =>
        Frame(new Dictionary<string, object?>
        {
            ["type"] = "unlock",
            ["uuid"] = uuid,
            ["key"] = key,
            ["lockUuid"] = string.IsNullOrEmpty(lockUuid) ? null : lockUuid,
            ["force"] = force ? true : null,
        });

    public static string UnlockRequestComposite(string uuid, IReadOnlyList<string> keys, string? lockUuid) =>
        Frame(new Dictionary<string, object?>
        {
            ["type"] = "unlock",
            ["uuid"] = uuid,
            ["keys"] = keys,
            ["lockUuid"] = string.IsNullOrEmpty(lockUuid) ? null : lockUuid,
        });

    public static string RwRequest(RequestType type, string uuid, string key) =>
        Frame(new Dictionary<string, object?> { ["type"] = ToWire(type), ["uuid"] = uuid, ["key"] = key });

    public static string LockInfoRequest(string uuid, string key) => RwRequest(RequestType.LockInfo, uuid, key);

    public static string LsRequest(string uuid) =>
        Frame(new Dictionary<string, object?> { ["type"] = "ls", ["uuid"] = uuid });

    public static string HeartbeatRequest(string uuid) =>
        Frame(new Dictionary<string, object?> { ["type"] = "heartbeat", ["uuid"] = uuid });

    public static Response DecodeResponse(string jsonLine)
    {
        using var doc = JsonDocument.Parse(jsonLine);
        var root = doc.RootElement;
        var type = ResponseTypeFromWire(GetString(root, "type") ?? "");
        var keys = GetStringArray(root, "keys");
        var fencingTokens = GetUInt64Map(root, "fencingTokens");

        return new Response(
            type,
            GetString(root, "uuid") ?? "",
            GetString(root, "key"),
            keys,
            GetBool(root, "acquired"),
            GetBool(root, "unlocked"),
            GetString(root, "lockUuid"),
            GetUInt64(root, "fencingToken"),
            fencingTokens,
            GetString(root, "error"));
    }

    private static string Frame(IDictionary<string, object?> fields)
    {
        var compact = fields.Where(kv => kv.Value is not null).ToDictionary(kv => kv.Key, kv => kv.Value);
        return JsonSerializer.Serialize(compact) + "\n";
    }

    private static string? GetString(JsonElement root, string name) =>
        root.TryGetProperty(name, out var value) && value.ValueKind == JsonValueKind.String ? value.GetString() : null;

    private static bool? GetBool(JsonElement root, string name) =>
        root.TryGetProperty(name, out var value) && value.ValueKind is JsonValueKind.True or JsonValueKind.False
            ? value.GetBoolean()
            : null;

    private static ulong? GetUInt64(JsonElement root, string name) =>
        root.TryGetProperty(name, out var value) && value.ValueKind == JsonValueKind.Number && value.TryGetUInt64(out var n)
            ? n
            : null;

    private static IReadOnlyList<string> GetStringArray(JsonElement root, string name)
    {
        if (!root.TryGetProperty(name, out var value) || value.ValueKind != JsonValueKind.Array)
        {
            return Array.Empty<string>();
        }

        return value.EnumerateArray().Select(item => item.GetString() ?? "").ToArray();
    }

    private static IReadOnlyDictionary<string, ulong> GetUInt64Map(JsonElement root, string name)
    {
        if (!root.TryGetProperty(name, out var value) || value.ValueKind != JsonValueKind.Object)
        {
            return new Dictionary<string, ulong>();
        }

        var map = new Dictionary<string, ulong>();
        foreach (var prop in value.EnumerateObject())
        {
            if (prop.Value.ValueKind == JsonValueKind.Number && prop.Value.TryGetUInt64(out var n))
            {
                map[prop.Name] = n;
            }
        }

        return map;
    }
}

