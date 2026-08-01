using System.ComponentModel;
using System.Runtime.InteropServices;
using Microsoft.Win32.SafeHandles;

namespace AjazzDoctor;

internal static class NativeMethods
{
    internal const uint DigcfPresent = 0x00000002;
    internal const uint DigcfDeviceInterface = 0x00000010;
    internal const uint GenericRead = 0x80000000;
    internal const uint GenericWrite = 0x40000000;
    internal const uint FileShareRead = 0x00000001;
    internal const uint FileShareWrite = 0x00000002;
    internal const uint OpenExisting = 3;
    internal const uint FileFlagOverlapped = 0x40000000;

    [DllImport("hid.dll")]
    internal static extern void HidD_GetHidGuid(out Guid hidGuid);

    [DllImport("hid.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    internal static extern bool HidD_GetAttributes(
        SafeFileHandle hidDeviceObject,
        ref HiddAttributes attributes);

    [DllImport("hid.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    internal static extern bool HidD_GetManufacturerString(
        SafeFileHandle hidDeviceObject,
        IntPtr buffer,
        int bufferLength);

    [DllImport("hid.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    internal static extern bool HidD_GetProductString(
        SafeFileHandle hidDeviceObject,
        IntPtr buffer,
        int bufferLength);

    [DllImport("hid.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    internal static extern bool HidD_GetSerialNumberString(
        SafeFileHandle hidDeviceObject,
        IntPtr buffer,
        int bufferLength);

    [DllImport("hid.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    internal static extern bool HidD_GetPreparsedData(
        SafeFileHandle hidDeviceObject,
        out IntPtr preparsedData);

    [DllImport("hid.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    internal static extern bool HidD_FreePreparsedData(IntPtr preparsedData);

    [DllImport("hid.dll")]
    internal static extern int HidP_GetCaps(
        IntPtr preparsedData,
        out HidpCaps capabilities);

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
        IntPtr deviceInterfaceDetailData,
        uint deviceInterfaceDetailDataSize,
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

    internal static SafeFileHandle Open(
        string path,
        uint desiredAccess,
        bool overlapped = false)
    {
        return CreateFile(
            path,
            desiredAccess,
            FileShareRead | FileShareWrite,
            IntPtr.Zero,
            OpenExisting,
            overlapped ? FileFlagOverlapped : 0,
            IntPtr.Zero);
    }

    internal static int LastError => Marshal.GetLastWin32Error();

    internal static string ErrorMessage(int code) =>
        new Win32Exception(code).Message;

    [StructLayout(LayoutKind.Sequential)]
    internal struct HiddAttributes
    {
        internal int Size;
        internal ushort VendorId;
        internal ushort ProductId;
        internal ushort VersionNumber;
    }

    [StructLayout(LayoutKind.Sequential)]
    internal struct SpDeviceInterfaceData
    {
        internal int Size;
        internal Guid InterfaceClassGuid;
        internal int Flags;
        internal IntPtr Reserved;
    }

    [StructLayout(LayoutKind.Sequential)]
    internal struct HidpCaps
    {
        internal ushort Usage;
        internal ushort UsagePage;
        internal ushort InputReportByteLength;
        internal ushort OutputReportByteLength;
        internal ushort FeatureReportByteLength;

        [MarshalAs(UnmanagedType.ByValArray, SizeConst = 17)]
        internal ushort[] Reserved;

        internal ushort NumberLinkCollectionNodes;
        internal ushort NumberInputButtonCaps;
        internal ushort NumberInputValueCaps;
        internal ushort NumberInputDataIndices;
        internal ushort NumberOutputButtonCaps;
        internal ushort NumberOutputValueCaps;
        internal ushort NumberOutputDataIndices;
        internal ushort NumberFeatureButtonCaps;
        internal ushort NumberFeatureValueCaps;
        internal ushort NumberFeatureDataIndices;
    }
}
