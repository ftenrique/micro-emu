#pragma once

#include <ntddk.h>
#include <wdf.h>
#include <vhf.h>
#include <wdmsec.h>

#include "../include/public.h"
#include "descriptor.h"

#define CODEX_OUTPUT_RING_CAPACITY 128u

typedef struct _CODEX_RING_ENTRY {
    ULONGLONG Sequence;
    ULONG Length;
    UCHAR Report[CODEX_MICRO_REPORT_BYTES];
} CODEX_RING_ENTRY, *PCODEX_RING_ENTRY;

typedef struct _DEVICE_CONTEXT {
    VHFHANDLE VhfHandle;
    WDFSPINLOCK RingLock;
    CODEX_RING_ENTRY Ring[CODEX_OUTPUT_RING_CAPACITY];
    ULONG RingHead;
    ULONG RingCount;
    ULONGLONG NextSequence;
    ULONGLONG OutputReportsReceived;
    ULONGLONG OutputReportsDropped;
    ULONGLONG InputReportsSubmitted;
    ULONGLONG InvalidReportsRejected;
} DEVICE_CONTEXT, *PDEVICE_CONTEXT;

WDF_DECLARE_CONTEXT_TYPE_WITH_NAME(DEVICE_CONTEXT, DeviceGetContext);

DRIVER_INITIALIZE DriverEntry;
EVT_WDF_DRIVER_DEVICE_ADD CodexEvtDeviceAdd;
EVT_WDF_OBJECT_CONTEXT_CLEANUP CodexEvtDeviceCleanup;
EVT_WDF_IO_QUEUE_IO_DEVICE_CONTROL CodexEvtIoDeviceControl;
EVT_VHF_ASYNC_OPERATION CodexEvtVhfWriteReport;
