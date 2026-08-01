using System.Text;
using System.Text.Json;

namespace ProtocolMonitor;

internal sealed class ProtocolDecoder
{
    private readonly List<byte> _buffer = new();

    internal IReadOnlyList<DecodedMessage> Feed(OutputReport record)
    {
        var report = record.Report;
        var offset = report.Length == 64 && report[0] == 6 ? 1 : 0;
        if (report.Length - offset < 2 || report[offset] != 2)
        {
            return Array.Empty<DecodedMessage>();
        }
        var length = report[offset + 1];
        if (length is 0 or > 61 || offset + 2 + length > report.Length)
        {
            return Array.Empty<DecodedMessage>();
        }
        _buffer.AddRange(report.AsSpan(offset + 2, length).ToArray());

        var messages = new List<DecodedMessage>();
        while (FindTerminator() is var end && end >= 0)
        {
            var bytes = _buffer.Take(end).ToArray();
            _buffer.RemoveRange(0, end + 2);
            try
            {
                var json = new UTF8Encoding(false, true).GetString(bytes);
                using var parsed = JsonDocument.Parse(json);
                if (parsed.RootElement.ValueKind == JsonValueKind.Object)
                {
                    messages.Add(
                        new DecodedMessage(
                            record.Sequence,
                            bytes.Length,
                            json,
                            parsed.RootElement.Clone()));
                }
            }
            catch (Exception exception)
                when (exception is DecoderFallbackException or JsonException)
            {
                // A malformed line is isolated; the next CRLF message can recover.
            }
        }
        return messages;
    }

    private int FindTerminator()
    {
        for (var index = 0; index < _buffer.Count - 1; index++)
        {
            if (_buffer[index] == 0x0d && _buffer[index + 1] == 0x0a)
            {
                return index;
            }
        }
        return -1;
    }
}

internal static class ProtocolEncoder
{
    private const int ChunkBytes = 61;

    internal static IReadOnlyList<byte[]> Frame(object message)
    {
        var json = JsonSerializer.Serialize(message);
        var payload = Encoding.UTF8.GetBytes(json + "\r\n");
        var reports = new List<byte[]>();
        for (var offset = 0; offset < payload.Length; offset += ChunkBytes)
        {
            var length = Math.Min(ChunkBytes, payload.Length - offset);
            var report = new byte[64];
            report[0] = 6;
            report[1] = 2;
            report[2] = checked((byte)length);
            payload.AsSpan(offset, length).CopyTo(report.AsSpan(3));
            reports.Add(report);
        }
        return reports;
    }
}

internal sealed record DecodedMessage(
    ulong LastSequence,
    int JsonBytes,
    string Json,
    JsonElement Root)
{
    internal string? Method
    {
        get
        {
            if (Root.TryGetProperty("m", out var compact) &&
                compact.ValueKind == JsonValueKind.String)
            {
                return compact.GetString();
            }
            if (Root.TryGetProperty("method", out var standard) &&
                standard.ValueKind == JsonValueKind.String)
            {
                return standard.GetString();
            }
            return null;
        }
    }

    internal JsonElement? Id =>
        Root.TryGetProperty("id", out var id) ? id.Clone() : null;
}
