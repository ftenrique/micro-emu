using System.Text.Json;

namespace ProtocolMonitor;

internal static class SelfTest
{
    internal static void Run()
    {
        var message = new
        {
            result = new
            {
                version = "0.4.1",
                profile_index = 0,
                layer_index = 0,
                battery = 100,
                is_charging = true,
            },
            id = 1,
        };
        var reports = ProtocolEncoder.Frame(message);
        Require(reports.Count > 1, "long response must fragment");
        Require(reports.All(report => report.Length == 64), "report length");
        Require(reports.All(report => report[0] == 6), "report id");

        var decoder = new ProtocolDecoder();
        var decoded = new List<DecodedMessage>();
        ulong sequence = 1;
        foreach (var report in reports)
        {
            decoded.AddRange(decoder.Feed(new OutputReport(sequence++, report)));
        }
        Require(decoded.Count == 1, "fragmented response must reassemble once");
        Require(decoded[0].Root.GetProperty("id").GetInt32() == 1, "id preserved");
        Require(
            decoded[0].Root.GetProperty("result").GetProperty("version").GetString()
                == "0.4.1",
            "status payload preserved");

        var eventReports = ProtocolEncoder.Frame(
            new { m = "v.oai.hid", p = new { k = "AG00", act = 1, ag = 0 } });
        Require(eventReports.Count == 1, "AG00 event should fit one report");

        Console.WriteLine(
            JsonSerializer.Serialize(
                new
                {
                    ok = true,
                    statusFragments = reports.Count,
                    ag00Fragments = eventReports.Count,
                    reportBytes = reports[0].Length,
                },
                new JsonSerializerOptions { WriteIndented = true }));
    }

    private static void Require(bool condition, string description)
    {
        if (!condition)
        {
            throw new InvalidOperationException($"Self-test failed: {description}.");
        }
    }
}
