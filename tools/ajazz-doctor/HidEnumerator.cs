using System.Runtime.InteropServices;

namespace AjazzDoctor;

internal static class HidEnumerator
{
    private const int ErrorNoMoreItems = 259;

    internal static IReadOnlyList<HidInterface> Enumerate()
    {
        NativeMethods.HidD_GetHidGuid(out var hidGuid);
        var deviceSet = NativeMethods.SetupDiGetClassDevs(
            ref hidGuid,
            IntPtr.Zero,
            IntPtr.Zero,
            NativeMethods.DigcfPresent | NativeMethods.DigcfDeviceInterface);
        if (deviceSet == new IntPtr(-1))
        {
            throw new InvalidOperationException(
                $"SetupDiGetClassDevs failed: {NativeMethods.ErrorMessage(NativeMethods.LastError)}");
        }

        var found = new List<HidInterface>();
        try
        {
            for (uint index = 0; ; index++)
            {
                var interfaceData = new NativeMethods.SpDeviceInterfaceData
                {
                    Size = Marshal.SizeOf<NativeMethods.SpDeviceInterfaceData>(),
                };
                if (!NativeMethods.SetupDiEnumDeviceInterfaces(
                        deviceSet,
                        IntPtr.Zero,
                        ref hidGuid,
                        index,
                        ref interfaceData))
                {
                    var error = NativeMethods.LastError;
                    if (error == ErrorNoMoreItems)
                    {
                        break;
                    }
                    throw new InvalidOperationException(
                        $"SetupDiEnumDeviceInterfaces failed: {NativeMethods.ErrorMessage(error)}");
                }

                _ = NativeMethods.SetupDiGetDeviceInterfaceDetail(
                    deviceSet,
                    ref interfaceData,
                    IntPtr.Zero,
                    0,
                    out var required,
                    IntPtr.Zero);
                if (required == 0)
                {
                    continue;
                }

                var detail = Marshal.AllocHGlobal(checked((int)required));
                try
                {
                    Marshal.WriteInt32(detail, IntPtr.Size == 8 ? 8 : 6);
                    if (!NativeMethods.SetupDiGetDeviceInterfaceDetail(
                            deviceSet,
                            ref interfaceData,
                            detail,
                            required,
                            out _,
                            IntPtr.Zero))
                    {
                        continue;
                    }
                    // SP_DEVICE_INTERFACE_DETAIL_DATA has a DWORD cbSize followed
                    // immediately by the first UTF-16 path character. cbSize is
                    // 8 on x64 because of native alignment, but the string itself
                    // still begins at byte 4.
                    const int pathOffset = 4;
                    var path = Marshal.PtrToStringUni(detail + pathOffset);
                    if (!string.IsNullOrWhiteSpace(path))
                    {
                        found.Add(Inspect(path));
                    }
                }
                finally
                {
                    Marshal.FreeHGlobal(detail);
                }
            }
        }
        finally
        {
            _ = NativeMethods.SetupDiDestroyDeviceInfoList(deviceSet);
        }
        return found;
    }

    private static HidInterface Inspect(string path)
    {
        using var metadata = NativeMethods.Open(path, 0);
        if (metadata.IsInvalid)
        {
            var code = NativeMethods.LastError;
            return HidInterface.Inaccessible(path, code, NativeMethods.ErrorMessage(code));
        }

        var attributes = new NativeMethods.HiddAttributes
        {
            Size = Marshal.SizeOf<NativeMethods.HiddAttributes>(),
        };
        var hasAttributes = NativeMethods.HidD_GetAttributes(metadata, ref attributes);
        var caps = ReadCapabilities(metadata);

        var read = ProbeAccess(path, NativeMethods.GenericRead);
        var write = ProbeAccess(path, NativeMethods.GenericWrite);
        var readWrite = ProbeAccess(
            path,
            NativeMethods.GenericRead | NativeMethods.GenericWrite);

        return new HidInterface(
            Path: path,
            VendorId: hasAttributes ? attributes.VendorId : null,
            ProductId: hasAttributes ? attributes.ProductId : null,
            VersionNumber: hasAttributes ? attributes.VersionNumber : null,
            Manufacturer: ReadString(metadata, NativeMethods.HidD_GetManufacturerString),
            Product: ReadString(metadata, NativeMethods.HidD_GetProductString),
            SerialNumber: ReadString(metadata, NativeMethods.HidD_GetSerialNumberString),
            UsagePage: caps?.UsagePage,
            Usage: caps?.Usage,
            InputReportBytes: caps?.InputReportByteLength,
            OutputReportBytes: caps?.OutputReportByteLength,
            FeatureReportBytes: caps?.FeatureReportByteLength,
            MetadataOpen: AccessProbe.Success,
            ReadOpen: read,
            WriteOpen: write,
            ReadWriteOpen: readWrite);
    }

    private static NativeMethods.HidpCaps? ReadCapabilities(
        Microsoft.Win32.SafeHandles.SafeFileHandle handle)
    {
        if (!NativeMethods.HidD_GetPreparsedData(handle, out var preparsed))
        {
            return null;
        }
        try
        {
            return NativeMethods.HidP_GetCaps(preparsed, out var caps) >= 0
                ? caps
                : null;
        }
        finally
        {
            _ = NativeMethods.HidD_FreePreparsedData(preparsed);
        }
    }

    private delegate bool HidStringReader(
        Microsoft.Win32.SafeHandles.SafeFileHandle handle,
        IntPtr buffer,
        int length);

    private static string? ReadString(
        Microsoft.Win32.SafeHandles.SafeFileHandle handle,
        HidStringReader reader)
    {
        const int bytes = 512;
        var buffer = Marshal.AllocHGlobal(bytes);
        try
        {
            for (var index = 0; index < bytes; index++)
            {
                Marshal.WriteByte(buffer, index, 0);
            }
            return reader(handle, buffer, bytes)
                ? Marshal.PtrToStringUni(buffer)?.TrimEnd('\0')
                : null;
        }
        finally
        {
            Marshal.FreeHGlobal(buffer);
        }
    }

    private static AccessProbe ProbeAccess(string path, uint access)
    {
        using var handle = NativeMethods.Open(path, access);
        if (!handle.IsInvalid)
        {
            return AccessProbe.Success;
        }
        var code = NativeMethods.LastError;
        return new AccessProbe(false, code, NativeMethods.ErrorMessage(code));
    }
}

internal sealed record AccessProbe(bool Opened, int? ErrorCode, string? Error)
{
    internal static readonly AccessProbe Success = new(true, null, null);
}

internal sealed record HidInterface(
    string Path,
    ushort? VendorId,
    ushort? ProductId,
    ushort? VersionNumber,
    string? Manufacturer,
    string? Product,
    string? SerialNumber,
    ushort? UsagePage,
    ushort? Usage,
    ushort? InputReportBytes,
    ushort? OutputReportBytes,
    ushort? FeatureReportBytes,
    AccessProbe MetadataOpen,
    AccessProbe ReadOpen,
    AccessProbe WriteOpen,
    AccessProbe ReadWriteOpen)
{
    internal static HidInterface Inaccessible(string path, int code, string error) =>
        new(
            path,
            null,
            null,
            null,
            null,
            null,
            null,
            null,
            null,
            null,
            null,
            null,
            new AccessProbe(false, code, error),
            new AccessProbe(false, code, error),
            new AccessProbe(false, code, error),
            new AccessProbe(false, code, error));
}
