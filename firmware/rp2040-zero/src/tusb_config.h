#pragma once

#ifdef __cplusplus
extern "C" {
#endif

#ifndef CFG_TUSB_MCU
#define CFG_TUSB_MCU OPT_MCU_RP2040
#endif
#ifndef CFG_TUSB_OS
#define CFG_TUSB_OS OPT_OS_NONE
#endif
#ifndef CFG_TUSB_RHPORT0_MODE
#define CFG_TUSB_RHPORT0_MODE (OPT_MODE_DEVICE | OPT_MODE_FULL_SPEED)
#endif

#define CFG_TUD_ENABLED 1
#define CFG_TUD_ENDPOINT0_SIZE 64

#define CFG_TUD_CDC 1
#define CFG_TUD_HID 2

#define CFG_TUD_CDC_RX_BUFSIZE 256
#define CFG_TUD_CDC_TX_BUFSIZE 512
#define CFG_TUD_CDC_EP_BUFSIZE 64
#define CFG_TUD_HID_EP_BUFSIZE 64

#ifdef __cplusplus
}
#endif
