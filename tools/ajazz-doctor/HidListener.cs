using System.Diagnostics;

namespace AjazzDoctor;

internal static class HidListener
{
    internal static async Task<CaptureResult> CaptureAsync(
        HidInterface device,
        int seconds)
    {
        if (!device.InputReportBytes.HasValue || device.InputReportBytes.Value == 0)
        {
            return new CaptureResult(
                device.Path,
                seconds,
                false,
                "Interface has no input reports.",
                Array.Empty<CapturedReport>());
        }

        var handle = NativeMethods.Open(
            device.Path,
            NativeMethods.GenericRead,
            overlapped: true);
        if (handle.IsInvalid)
        {
            var error = NativeMethods.LastError;
            handle.Dispose();
            return new CaptureResult(
                device.Path,
                seconds,
                false,
                NativeMethods.ErrorMessage(error),
                Array.Empty<CapturedReport>());
        }

        await using var stream = new FileStream(
            handle,
            FileAccess.Read,
            device.InputReportBytes.Value,
            isAsync: true);
        using var cancellation = new CancellationTokenSource(
            TimeSpan.FromSeconds(seconds));
        var stopwatch = Stopwatch.StartNew();
        var reports = new List<CapturedReport>();

        try
        {
            while (!cancellation.IsCancellationRequested && reports.Count < 256)
            {
                var buffer = new byte[device.InputReportBytes.Value];
                var count = await stream.ReadAsync(
                    buffer.AsMemory(),
                    cancellation.Token);
                if (count == 0)
                {
                    break;
                }
                reports.Add(
                    new CapturedReport(
                        stopwatch.ElapsedMilliseconds,
                        count,
                        Convert.ToHexString(buffer.AsSpan(0, count))));
            }
        }
        catch (OperationCanceledException) when (cancellation.IsCancellationRequested)
        {
            // The capture window ended normally.
        }
        catch (Exception exception)
        {
            return new CaptureResult(
                device.Path,
                seconds,
                false,
                exception.Message,
                reports);
        }

        return new CaptureResult(
            device.Path,
            seconds,
            true,
            null,
            reports);
    }
}

internal sealed record CaptureResult(
    string Path,
    int DurationSeconds,
    bool Completed,
    string? Error,
    IReadOnlyList<CapturedReport> Reports);

internal sealed record CapturedReport(
    long ElapsedMilliseconds,
    int Bytes,
    string Hex);
