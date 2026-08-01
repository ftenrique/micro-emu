using System.Text.Json;

namespace ProtocolMonitor;

internal static class Program
{
    private static int Main(string[] args)
    {
        try
        {
            var options = Options.Parse(args);
            if (options.SelfTest)
            {
                SelfTest.Run();
                return 0;
            }
            using var channel = DriverChannel.Open();

            if (options.ShowStats)
            {
                Console.WriteLine(
                    JsonSerializer.Serialize(
                        channel.GetStats(),
                        new JsonSerializerOptions { WriteIndented = true }));
            }

            if (options.EmitKey is not null)
            {
                EmitKey(channel, options.EmitKey);
                Console.WriteLine($"Emitted {options.EmitKey} down/up.");
            }

            if (options.ServeSeconds > 0)
            {
                Serve(channel, options);
            }
            return 0;
        }
        catch (ArgumentException exception)
        {
            Console.Error.WriteLine(exception.Message);
            PrintUsage();
            return 64;
        }
        catch (Exception exception)
        {
            Console.Error.WriteLine($"protocol-monitor failed: {exception.Message}");
            return 1;
        }
    }

    private static void Serve(DriverChannel channel, Options options)
    {
        var decoder = new ProtocolDecoder();
        using var capture = options.CapturePath is null
            ? null
            : new StreamWriter(
                Path.GetFullPath(options.CapturePath),
                append: true);
        var end = DateTimeOffset.UtcNow.AddSeconds(options.ServeSeconds);
        Console.WriteLine(
            $"Monitoring for {options.ServeSeconds}s; device.status auto-response is enabled.");

        while (DateTimeOffset.UtcNow < end)
        {
            var report = channel.TryGetOutput();
            if (report is null)
            {
                Thread.Sleep(25);
                continue;
            }
            foreach (var message in decoder.Feed(report))
            {
                var summary = new
                {
                    at = DateTimeOffset.UtcNow,
                    sequence = message.LastSequence,
                    method = message.Method,
                    id = message.Id,
                    jsonBytes = message.JsonBytes,
                };
                Console.WriteLine(JsonSerializer.Serialize(summary));
                if (options.Verbose)
                {
                    Console.WriteLine(message.Json);
                }
                if (capture is not null)
                {
                    capture.WriteLine(
                        JsonSerializer.Serialize(
                            new
                            {
                                at = DateTimeOffset.UtcNow,
                                sequence = message.LastSequence,
                                json = message.Root,
                            }));
                    capture.Flush();
                }
                Respond(channel, message);
            }
        }
        Console.WriteLine(
            JsonSerializer.Serialize(
                channel.GetStats(),
                new JsonSerializerOptions { WriteIndented = true }));
    }

    private static void Respond(DriverChannel channel, DecodedMessage message)
    {
        if (message.Method == "device.status" && message.Id is JsonElement id)
        {
            Send(
                channel,
                new
                {
                    result = new
                    {
                        version = "0.4.1",
                        profile_index = 0,
                        layer_index = 0,
                        battery = 100,
                        is_charging = true,
                    },
                    id,
                });
            return;
        }

        if (message.Id is JsonElement unknownId &&
            message.Method is not null &&
            !message.Method.StartsWith("v.oai.", StringComparison.Ordinal))
        {
            Send(
                channel,
                new
                {
                    error = new { code = 404, message = "Method not found" },
                    id = unknownId,
                });
        }
    }

    private static void EmitKey(DriverChannel channel, string key)
    {
        var agent = int.Parse(key.AsSpan(2, 2));
        Send(
            channel,
            new { m = "v.oai.hid", p = new { k = key, act = 1, ag = agent } });
        Thread.Sleep(60);
        Send(
            channel,
            new { m = "v.oai.hid", p = new { k = key, act = 0, ag = agent } });
    }

    private static void Send(DriverChannel channel, object message)
    {
        foreach (var report in ProtocolEncoder.Frame(message))
        {
            channel.SendInput(report);
        }
    }

    private static void PrintUsage()
    {
        Console.Error.WriteLine(
            "Usage: protocol-monitor [--serve 60] [--stats] [--emit AG00] " +
            "[--capture file.jsonl] [--verbose] [--self-test]");
    }
}

internal sealed record Options(
    int ServeSeconds,
    bool ShowStats,
    string? EmitKey,
    string? CapturePath,
    bool Verbose,
    bool SelfTest)
{
    internal static Options Parse(string[] args)
    {
        var serve = 0;
        var stats = false;
        string? emit = null;
        string? capture = null;
        var verbose = false;
        var selfTest = false;

        for (var index = 0; index < args.Length; index++)
        {
            switch (args[index])
            {
            case "--serve":
                if (++index >= args.Length ||
                    !int.TryParse(args[index], out serve) ||
                    serve is < 1 or > 3600)
                {
                    throw new ArgumentException(
                        "--serve requires 1-3600 seconds.");
                }
                break;
            case "--stats":
                stats = true;
                break;
            case "--emit":
                if (++index >= args.Length ||
                    !System.Text.RegularExpressions.Regex.IsMatch(
                        args[index],
                        "^AG0[0-5]$"))
                {
                    throw new ArgumentException(
                        "--emit requires AG00 through AG05.");
                }
                emit = args[index];
                break;
            case "--capture":
                if (++index >= args.Length)
                {
                    throw new ArgumentException("--capture requires a path.");
                }
                capture = args[index];
                break;
            case "--verbose":
                verbose = true;
                break;
            case "--self-test":
                selfTest = true;
                break;
            default:
                throw new ArgumentException($"Unknown argument: {args[index]}");
            }
        }

        if (serve == 0 && !stats && emit is null && !selfTest)
        {
            serve = 60;
        }
        return new Options(serve, stats, emit, capture, verbose, selfTest);
    }
}
