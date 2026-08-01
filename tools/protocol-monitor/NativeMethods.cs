using System.ComponentModel;
using System.Runtime.InteropServices;
using Microsoft.Win32.SafeHandles;

namespace ProtocolMonitor;

internal static class NativeMethods
{
    internal static readonly Guid ControlInterfaceGuid =
        new("7A50A0E8-289F-4A72-9BC4-11A40FC1A63C");

    internal const uint DigcfPresent = 0x00000002;
    internal const uint DigcfDeviceInterface = 0x00000010;
    internal const uint GenericRead = 0x80000000;
    internal const uint GenericWrite = 0x40000000;
    internal const uint FileShareRead = 0x00000001;
    internal const uint FileShareWrite = 0x00000002;
    internal const uint OpenExisting = 3;
    internal const int ErrorNoMoreItems = 259;

    internal const uint IoctlGetOutputReport = 0x0022E000;
    internal const uint IoctlSendInputReport = 0x0022E004;
    internal const uint IoctlGetStats = 0x0022E008;

    [DllImport("setupapi.dll", SetLastError = true)]
    internal static extern IntPtr SetupDiGetClassDevs(
        ref Guid classGuid,
        IntPtr enumerator,
        IntPtr hwndParent,
        uint flags);

    [DllImport("setupapi.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    internal static extern bool SetupDiEnumDeviceInterfaces(
        IntPtr deviceInfoSet,
        IntPtr deviceInfoData,
        ref Guid interfaceClassGuid,
        uint memberIndex,
        ref SpDeviceInterfaceData deviceInterfaceData);

    [DllImport("setupapi.dll", SetLastError = true, CharSet = CharSet.Auto)]
    [return: MarshalAs(UnmanagedType.Bool)]
    internal static extern bool SetupDiGetDeviceInterfaceDetail(
        IntPtr deviceInfoSet,
        ref SpDeviceInterfaceData deviceInterfaceData,
        IntPtr detailData,
        uint detailDataSize,
        out uint requiredSize,
        IntPtr deviceInfoData);

    [DllImport("setupapi.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    internal static extern bool SetupDiDestroyDeviceInfoList(IntPtr deviceInfoSet);

    [DllImport("kernel32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
    internal static extern SafeFileHandle CreateFile(
        string fileName,
        uint desiredAccess,
        uint shareMode,
        IntPtr securityAttributes,
        uint creationDisposition,
        uint flagsAndAttributes,
        IntPtr templateFile);

    [DllImport("kernel32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    internal static extern bool DeviceIoControl(
        SafeFileHandle device,
        uint controlCode,
        byte[]? input,
        int inputLength,
        byte[]? output,
        int outputLength,
        out int bytesReturned,
        IntPtr overlapped);

    internal static string ErrorMessage(int code) =>
        new Win32Exception(code).Message;

    [StructLayout(LayoutKind.Sequential)]
    internal struct SpDeviceInterfaceData
    {
        internal int Size;
        internal Guid InterfaceClassGuid;
        internal int Flags;
        internal IntPtr Reserved;
    }
}
