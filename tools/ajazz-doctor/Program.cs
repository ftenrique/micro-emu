using System.Globalization;
using System.Text.Json;
using System.Text.Json.Serialization;

namespace AjazzDoctor;

internal static class Program
{
    private static async Task<int> Main(string[] args)
    {
        try
        {
            var options = Options.Parse(args);
            var devices = HidEnumerator.Enumerate()
                .Where(device =>
                    (!options.VendorId.HasValue || device.VendorId == options.VendorId) &&
                    (!options.ProductId.HasValue || device.ProductId == options.ProductId))
                .OrderBy(device => device.UsagePage)
                .ThenBy(device => device.Usage)
                .ToArray();

            CaptureResult? capture = null;
            if (options.ListenSeconds > 0)
            {
                var target = devices.FirstOrDefault(
                    device =>
                        device.UsagePage is >= 0xff00 &&
                        device.ReadOpen.Opened &&
                        device.InputReportBytes is > 0);
                capture = target is null
                    ? new CaptureResult(
                        string.Empty,
                        options.ListenSeconds,
                        false,
                        "No readable vendor-defined HID interface matched the filter.",
                        Array.Empty<CapturedReport>())
                    : await HidListener.CaptureAsync(target, options.ListenSeconds);
            }

            var report = new
            {
                schemaVersion = 1,
                capturedAtUtc = DateTimeOffset.UtcNow,
                readOnlyProbe = true,
                filter = new
                {
                    vendorId = Hex(options.VendorId),
                    productId = Hex(options.ProductId),
                },
                summary = new
                {
                    interfaceCount = devices.Length,
                    vendorDefinedCount = devices.Count(
                        device => device.UsagePage is >= 0xff00),
                    readOpenCount = devices.Count(device => device.ReadOpen.Opened),
                    writeOpenCount = devices.Count(device => device.WriteOpen.Opened),
                    readWriteOpenCount = devices.Count(
                        device => device.ReadWriteOpen.Opened),
                },
                interfaces = devices.Select(ToOutput),
                capture,
            };

            var json = JsonSerializer.Serialize(
                report,
                new JsonSerializerOptions
                {
                    WriteIndented = true,
                    DefaultIgnoreCondition = JsonIgnoreCondition.WhenWritingNull,
                });
            Console.WriteLine(json);
            if (options.OutputPath is not null)
            {
                var fullPath = Path.GetFullPath(options.OutputPath);
                Directory.CreateDirectory(
                    Path.GetDirectoryName(fullPath)
                    ?? throw new InvalidOperationException("Output path has no directory."));
                File.WriteAllText(fullPath, json + Environment.NewLine);
            }
            return devices.Length == 0 ? 2 : 0;
        }
        catch (ArgumentException exception)
        {
            Console.Error.WriteLine(exception.Message);
            Console.Error.WriteLine(
                "Usage: ajazz-doctor [--vid 04b4] [--pid 1007] [--listen 10] [--output file.json]");
            return 64;
        }
        catch (Exception exception)
        {
            Console.Error.WriteLine($"ajazz-doctor failed: {exception.Message}");
            return 1;
        }
    }

    private static object ToOutput(HidInterface device) => new
    {
        path = device.Path,
        vendorId = Hex(device.VendorId),
        productId = Hex(device.ProductId),
        versionNumber = Hex(device.VersionNumber),
        device.Manufacturer,
        device.Product,
        device.SerialNumber,
        usagePage = Hex(device.UsagePage),
        usage = Hex(device.Usage),
        device.InputReportBytes,
        device.OutputReportBytes,
        device.FeatureReportBytes,
        access = new
        {
            metadata = device.MetadataOpen,
            read = device.ReadOpen,
            write = device.WriteOpen,
            readWrite = device.ReadWriteOpen,
        },
    };

    private static string? Hex(ushort? value) =>
        value.HasValue ? $"0x{value.Value:X4}" : null;
}

internal sealed record Options(
    ushort? VendorId,
    ushort? ProductId,
    string? OutputPath,
    int ListenSeconds)
{
    internal static Options Parse(string[] args)
    {
        ushort? vid = null;
        ushort? pid = null;
        string? output = null;
        var listenSeconds = 0;
        for (var index = 0; index < args.Length; index++)
        {
            var name = args[index];
            if (name is "--vid" or "--pid" or "--output" or "--listen")
            {
                if (++index >= args.Length)
                {
                    throw new ArgumentException($"{name} requires a value.");
                }
                if (name == "--output")
                {
                    output = args[index];
                }
                else if (name == "--listen")
                {
                    if (!int.TryParse(args[index], out listenSeconds) ||
                        listenSeconds is < 1 or > 60)
                    {
                        throw new ArgumentException(
                            "--listen must be a number of seconds from 1 to 60.");
                    }
                }
                else
                {
                    var parsed = ParseHex(args[index], name);
                    if (name == "--vid") vid = parsed;
                    if (name == "--pid") pid = parsed;
                }
                continue;
            }
            throw new ArgumentException($"Unknown argument: {name}");
        }
        return new Options(vid, pid, output, listenSeconds);
    }

    private static ushort ParseHex(string value, string name)
    {
        var normalized = value.StartsWith("0x", StringComparison.OrdinalIgnoreCase)
            ? value[2..]
            : value;
        if (!ushort.TryParse(
                normalized,
                NumberStyles.AllowHexSpecifier,
                CultureInfo.InvariantCulture,
                out var parsed))
        {
            throw new ArgumentException($"{name} must be a 16-bit hexadecimal value.");
        }
        return parsed;
    }
}
