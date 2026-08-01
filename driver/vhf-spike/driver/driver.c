#include "driver.h"

static NTSTATUS
CodexCreateVhf(
    _In_ WDFDEVICE Device,
    _Inout_ PDEVICE_CONTEXT Context
    );

static VOID
CodexCaptureOutputReport(
    _Inout_ PDEVICE_CONTEXT Context,
    _In_reads_bytes_(Length) const UCHAR* Report,
    _In_ ULONG Length
    );

static BOOLEAN
CodexPopOutputReport(
    _Inout_ PDEVICE_CONTEXT Context,
    _Out_ PCODEX_REPORT_RECORD Record
    );

_Use_decl_annotations_
NTSTATUS
DriverEntry(
    PDRIVER_OBJECT DriverObject,
    PUNICODE_STRING RegistryPath
    )
{
    WDF_DRIVER_CONFIG config;

    ExInitializeDriverRuntime(DrvRtPoolNxOptIn);
    WDF_DRIVER_CONFIG_INIT(&config, CodexEvtDeviceAdd);

    return WdfDriverCreate(
        DriverObject,
        RegistryPath,
        WDF_NO_OBJECT_ATTRIBUTES,
        &config,
        WDF_NO_HANDLE);
}

_Use_decl_annotations_
NTSTATUS
CodexEvtDeviceAdd(
    WDFDRIVER Driver,
    PWDFDEVICE_INIT DeviceInit
    )
{
    WDF_OBJECT_ATTRIBUTES attributes;
    WDF_IO_QUEUE_CONFIG queueConfig;
    WDFDEVICE device;
    PDEVICE_CONTEXT context;
    NTSTATUS status;
    UNREFERENCED_PARAMETER(Driver);
    PAGED_CODE();

    WdfDeviceInitSetDeviceType(DeviceInit, FILE_DEVICE_UNKNOWN);
    WdfDeviceInitSetExclusive(DeviceInit, FALSE);

    status = WdfDeviceInitAssignSDDLString(
        DeviceInit,
        &SDDL_DEVOBJ_SYS_ALL_ADM_RWX_WORLD_RW_RES_R);
    if (!NT_SUCCESS(status)) {
        return status;
    }

    WDF_OBJECT_ATTRIBUTES_INIT_CONTEXT_TYPE(&attributes, DEVICE_CONTEXT);
    attributes.EvtCleanupCallback = CodexEvtDeviceCleanup;

    status = WdfDeviceCreate(&DeviceInit, &attributes, &device);
    if (!NT_SUCCESS(status)) {
        return status;
    }

    context = DeviceGetContext(device);
    RtlZeroMemory(context, sizeof(*context));
    context->NextSequence = 1;

    status = WdfSpinLockCreate(WDF_NO_OBJECT_ATTRIBUTES, &context->RingLock);
    if (!NT_SUCCESS(status)) {
        return status;
    }

    status = WdfDeviceCreateDeviceInterface(
        device,
        &GUID_DEVINTERFACE_CODEX_MICRO_CONTROL,
        NULL);
    if (!NT_SUCCESS(status)) {
        return status;
    }

    WDF_IO_QUEUE_CONFIG_INIT_DEFAULT_QUEUE(
        &queueConfig,
        WdfIoQueueDispatchParallel);
    queueConfig.EvtIoDeviceControl = CodexEvtIoDeviceControl;

    status = WdfIoQueueCreate(
        device,
        &queueConfig,
        WDF_NO_OBJECT_ATTRIBUTES,
        WDF_NO_HANDLE);
    if (!NT_SUCCESS(status)) {
        return status;
    }

    return CodexCreateVhf(device, context);
}

static NTSTATUS
CodexCreateVhf(
    _In_ WDFDEVICE Device,
    _Inout_ PDEVICE_CONTEXT Context
    )
{
    VHF_CONFIG config;
    NTSTATUS status;

    PAGED_CODE();

    VHF_CONFIG_INIT(
        &config,
        WdfDeviceWdmGetDeviceObject(Device),
        g_CodexMicroReportDescriptorLength,
        g_CodexMicroReportDescriptor);
    config.VhfClientContext = Context;
    config.VendorID = 0x303A;
    config.ProductID = 0x8360;
    config.VersionNumber = 0x0001;
    config.EvtVhfAsyncOperationWriteReport = CodexEvtVhfWriteReport;

    status = VhfCreate(&config, &Context->VhfHandle);
    if (!NT_SUCCESS(status)) {
        Context->VhfHandle = NULL;
        return status;
    }

    status = VhfStart(Context->VhfHandle);
    if (!NT_SUCCESS(status)) {
        VhfDelete(Context->VhfHandle, TRUE);
        Context->VhfHandle = NULL;
    }
    return status;
}

_Use_decl_annotations_
VOID
CodexEvtDeviceCleanup(
    WDFOBJECT DeviceObject
    )
{
    PDEVICE_CONTEXT context;

    PAGED_CODE();
    context = DeviceGetContext((WDFDEVICE)DeviceObject);
    if (context->VhfHandle != NULL) {
        VhfDelete(context->VhfHandle, TRUE);
        context->VhfHandle = NULL;
    }
}

_Use_decl_annotations_
VOID
CodexEvtVhfWriteReport(
    PVOID VhfClientContext,
    VHFOPERATIONHANDLE VhfOperationHandle,
    PVOID VhfOperationContext,
    PHID_XFER_PACKET HidTransferPacket
    )
{
    PDEVICE_CONTEXT context = (PDEVICE_CONTEXT)VhfClientContext;
    UCHAR normalized[CODEX_MICRO_REPORT_BYTES];
    NTSTATUS status = STATUS_INVALID_BUFFER_SIZE;

    UNREFERENCED_PARAMETER(VhfOperationContext);
    RtlZeroMemory(normalized, sizeof(normalized));

    if (HidTransferPacket != NULL &&
        HidTransferPacket->reportBuffer != NULL &&
        HidTransferPacket->reportId == CODEX_MICRO_REPORT_ID) {

        if (HidTransferPacket->reportBufferLen == CODEX_MICRO_REPORT_BYTES &&
            HidTransferPacket->reportBuffer[0] == CODEX_MICRO_REPORT_ID) {
            RtlCopyMemory(
                normalized,
                HidTransferPacket->reportBuffer,
                CODEX_MICRO_REPORT_BYTES);
            status = STATUS_SUCCESS;
        } else if (HidTransferPacket->reportBufferLen ==
                   CODEX_MICRO_REPORT_BYTES - 1) {
            normalized[0] = CODEX_MICRO_REPORT_ID;
            RtlCopyMemory(
                normalized + 1,
                HidTransferPacket->reportBuffer,
                CODEX_MICRO_REPORT_BYTES - 1);
            status = STATUS_SUCCESS;
        }
    }

    if (NT_SUCCESS(status)) {
        CodexCaptureOutputReport(
            context,
            normalized,
            CODEX_MICRO_REPORT_BYTES);
    } else {
        InterlockedIncrement64(
            (volatile LONG64*)&context->InvalidReportsRejected);
    }

    (VOID)VhfAsyncOperationComplete(VhfOperationHandle, status);
}

static VOID
CodexCaptureOutputReport(
    _Inout_ PDEVICE_CONTEXT Context,
    _In_reads_bytes_(Length) const UCHAR* Report,
    _In_ ULONG Length
    )
{
    ULONG slot;
    PCODEX_RING_ENTRY entry;

    WdfSpinLockAcquire(Context->RingLock);

    if (Context->RingCount == CODEX_OUTPUT_RING_CAPACITY) {
        Context->RingHead =
            (Context->RingHead + 1) % CODEX_OUTPUT_RING_CAPACITY;
        Context->RingCount--;
        Context->OutputReportsDropped++;
    }

    slot = (Context->RingHead + Context->RingCount) %
        CODEX_OUTPUT_RING_CAPACITY;
    entry = &Context->Ring[slot];
    entry->Sequence = Context->NextSequence++;
    entry->Length = Length;
    RtlCopyMemory(entry->Report, Report, Length);
    Context->RingCount++;
    Context->OutputReportsReceived++;

    WdfSpinLockRelease(Context->RingLock);
}

static BOOLEAN
CodexPopOutputReport(
    _Inout_ PDEVICE_CONTEXT Context,
    _Out_ PCODEX_REPORT_RECORD Record
    )
{
    PCODEX_RING_ENTRY entry;
    BOOLEAN found = FALSE;

    WdfSpinLockAcquire(Context->RingLock);
    if (Context->RingCount > 0) {
        entry = &Context->Ring[Context->RingHead];
        Record->Sequence = entry->Sequence;
        Record->Length = entry->Length;
        RtlCopyMemory(Record->Report, entry->Report, entry->Length);
        Context->RingHead =
            (Context->RingHead + 1) % CODEX_OUTPUT_RING_CAPACITY;
        Context->RingCount--;
        found = TRUE;
    }
    WdfSpinLockRelease(Context->RingLock);

    return found;
}

_Use_decl_annotations_
VOID
CodexEvtIoDeviceControl(
    WDFQUEUE Queue,
    WDFREQUEST Request,
    size_t OutputBufferLength,
    size_t InputBufferLength,
    ULONG IoControlCode
    )
{
    WDFDEVICE device = WdfIoQueueGetDevice(Queue);
    PDEVICE_CONTEXT context = DeviceGetContext(device);
    NTSTATUS status = STATUS_INVALID_DEVICE_REQUEST;
    size_t information = 0;

    UNREFERENCED_PARAMETER(OutputBufferLength);
    UNREFERENCED_PARAMETER(InputBufferLength);

    switch (IoControlCode) {
    case IOCTL_CODEX_GET_OUTPUT_REPORT:
    {
        PCODEX_REPORT_RECORD record = NULL;
        status = WdfRequestRetrieveOutputBuffer(
            Request,
            sizeof(*record),
            (PVOID*)&record,
            NULL);
        if (NT_SUCCESS(status)) {
            RtlZeroMemory(record, sizeof(*record));
            if (CodexPopOutputReport(context, record)) {
                information = sizeof(*record);
            } else {
                status = STATUS_NO_MORE_ENTRIES;
            }
        }
        break;
    }

    case IOCTL_CODEX_SEND_INPUT_REPORT:
    {
        PUCHAR report = NULL;
        HID_XFER_PACKET packet;

        status = WdfRequestRetrieveInputBuffer(
            Request,
            CODEX_MICRO_REPORT_BYTES,
            (PVOID*)&report,
            NULL);
        if (!NT_SUCCESS(status)) {
            break;
        }
        if (report[0] != CODEX_MICRO_REPORT_ID ||
            context->VhfHandle == NULL) {
            InterlockedIncrement64(
                (volatile LONG64*)&context->InvalidReportsRejected);
            status = report[0] != CODEX_MICRO_REPORT_ID
                ? STATUS_INVALID_PARAMETER
                : STATUS_DEVICE_NOT_READY;
            break;
        }

        packet.reportBuffer = report;
        packet.reportBufferLen = CODEX_MICRO_REPORT_BYTES;
        packet.reportId = CODEX_MICRO_REPORT_ID;
        status = VhfReadReportSubmit(context->VhfHandle, &packet);
        if (NT_SUCCESS(status)) {
            InterlockedIncrement64(
                (volatile LONG64*)&context->InputReportsSubmitted);
        }
        break;
    }

    case IOCTL_CODEX_GET_STATS:
    {
        PCODEX_DRIVER_STATS stats = NULL;
        status = WdfRequestRetrieveOutputBuffer(
            Request,
            sizeof(*stats),
            (PVOID*)&stats,
            NULL);
        if (NT_SUCCESS(status)) {
            WdfSpinLockAcquire(context->RingLock);
            stats->OutputReportsReceived = context->OutputReportsReceived;
            stats->OutputReportsDropped = context->OutputReportsDropped;
            stats->InputReportsSubmitted = context->InputReportsSubmitted;
            stats->InvalidReportsRejected = context->InvalidReportsRejected;
            stats->QueuedOutputReports = context->RingCount;
            stats->RingCapacity = CODEX_OUTPUT_RING_CAPACITY;
            WdfSpinLockRelease(context->RingLock);
            information = sizeof(*stats);
        }
        break;
    }
    }

    WdfRequestCompleteWithInformation(Request, status, information);
}
