using System.Runtime.InteropServices;
using Microsoft.Win32.SafeHandles;

namespace ProtocolMonitor;

internal sealed class DriverChannel : IDisposable
{
    private readonly SafeFileHandle _handle;

    private DriverChannel(SafeFileHandle handle)
    {
        _handle = handle;
    }

    internal static DriverChannel Open()
    {
        var path = FindInterfacePath()
            ?? throw new InvalidOperationException(
                "The Codex Micro VHF control interface is not present. Install and start the driver first.");
        var handle = NativeMethods.CreateFile(
            path,
            NativeMethods.GenericRead | NativeMethods.GenericWrite,
            NativeMethods.FileShareRead | NativeMethods.FileShareWrite,
            IntPtr.Zero,
            NativeMethods.OpenExisting,
            0,
            IntPtr.Zero);
        if (handle.IsInvalid)
        {
            var error = Marshal.GetLastWin32Error();
            handle.Dispose();
            throw new InvalidOperationException(
                $"Opening the driver control interface failed: {NativeMethods.ErrorMessage(error)}");
        }
        return new DriverChannel(handle);
    }

    internal OutputReport? TryGetOutput()
    {
        var output = new byte[76];
        if (!NativeMethods.DeviceIoControl(
                _handle,
                NativeMethods.IoctlGetOutputReport,
                null,
                0,
                output,
                output.Length,
                out var returned,
                IntPtr.Zero))
        {
            var error = Marshal.GetLastWin32Error();
            if (error == NativeMethods.ErrorNoMoreItems)
            {
                return null;
            }
            throw new InvalidOperationException(
                $"GET_OUTPUT_REPORT failed: {NativeMethods.ErrorMessage(error)} ({error}).");
        }
        if (returned != output.Length)
        {
            throw new InvalidOperationException(
                $"Driver returned {returned} bytes for a 76-byte output record.");
        }
        var sequence = BitConverter.ToUInt64(output, 0);
        var length = BitConverter.ToUInt32(output, 8);
        if (length is 0 or > 64)
        {
            throw new InvalidOperationException(
                $"Driver returned invalid report length {length}.");
        }
        return new OutputReport(sequence, output.AsSpan(12, (int)length).ToArray());
    }

    internal void SendInput(byte[] report)
    {
        if (report.Length != 64 || report[0] != 6)
        {
            throw new ArgumentException(
                "Input report must be 64 bytes and start with Report ID 6.",
                nameof(report));
        }
        if (!NativeMethods.DeviceIoControl(
                _handle,
                NativeMethods.IoctlSendInputReport,
                report,
                report.Length,
                null,
                0,
                out _,
                IntPtr.Zero))
        {
            var error = Marshal.GetLastWin32Error();
            throw new InvalidOperationException(
                $"SEND_INPUT_REPORT failed: {NativeMethods.ErrorMessage(error)} ({error}).");
        }
    }

    internal DriverStats GetStats()
    {
        var output = new byte[40];
        if (!NativeMethods.DeviceIoControl(
                _handle,
                NativeMethods.IoctlGetStats,
                null,
                0,
                output,
                output.Length,
                out var returned,
                IntPtr.Zero))
        {
            var error = Marshal.GetLastWin32Error();
            throw new InvalidOperationException(
                $"GET_STATS failed: {NativeMethods.ErrorMessage(error)} ({error}).");
        }
        if (returned != output.Length)
        {
            throw new InvalidOperationException(
                $"Driver returned {returned} bytes for 40-byte stats.");
        }
        return new DriverStats(
            BitConverter.ToUInt64(output, 0),
            BitConverter.ToUInt64(output, 8),
            BitConverter.ToUInt64(output, 16),
            BitConverter.ToUInt64(output, 24),
            BitConverter.ToUInt32(output, 32),
            BitConverter.ToUInt32(output, 36));
    }

    public void Dispose()
    {
        _handle.Dispose();
    }

    private static string? FindInterfacePath()
    {
        var guid = NativeMethods.ControlInterfaceGuid;
        var deviceSet = NativeMethods.SetupDiGetClassDevs(
            ref guid,
            IntPtr.Zero,
            IntPtr.Zero,
            NativeMethods.DigcfPresent | NativeMethods.DigcfDeviceInterface);
        if (deviceSet == new IntPtr(-1))
        {
            return null;
        }
        try
        {
            var data = new NativeMethods.SpDeviceInterfaceData
            {
                Size = Marshal.SizeOf<NativeMethods.SpDeviceInterfaceData>(),
            };
            if (!NativeMethods.SetupDiEnumDeviceInterfaces(
                    deviceSet,
                    IntPtr.Zero,
                    ref guid,
                    0,
                    ref data))
            {
                return null;
            }
            _ = NativeMethods.SetupDiGetDeviceInterfaceDetail(
                deviceSet,
                ref data,
                IntPtr.Zero,
                0,
                out var required,
                IntPtr.Zero);
            if (required == 0)
            {
                return null;
            }
            var detail = Marshal.AllocHGlobal(checked((int)required));
            try
            {
                Marshal.WriteInt32(detail, IntPtr.Size == 8 ? 8 : 6);
                if (!NativeMethods.SetupDiGetDeviceInterfaceDetail(
                        deviceSet,
                        ref data,
                        detail,
                        required,
                        out _,
                        IntPtr.Zero))
                {
                    return null;
                }
                return Marshal.PtrToStringUni(detail + 4);
            }
            finally
            {
                Marshal.FreeHGlobal(detail);
            }
        }
        finally
        {
            _ = NativeMethods.SetupDiDestroyDeviceInfoList(deviceSet);
        }
    }
}

internal sealed record OutputReport(ulong Sequence, byte[] Report);

internal sealed record DriverStats(
    ulong OutputReportsReceived,
    ulong OutputReportsDropped,
    ulong InputReportsSubmitted,
    ulong InvalidReportsRejected,
    uint QueuedOutputReports,
    uint RingCapacity);
