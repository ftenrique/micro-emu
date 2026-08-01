cmake_minimum_required(VERSION 3.13)

if(NOT DEFINED PICO_SDK_PATH)
    if(DEFINED ENV{PICO_SDK_PATH})
        set(PICO_SDK_PATH $ENV{PICO_SDK_PATH})
    else()
        message(FATAL_ERROR "Set PICO_SDK_PATH to a local Raspberry Pi Pico SDK checkout.")
    endif()
endif()

include("${PICO_SDK_PATH}/external/pico_sdk_import.cmake")

project(codex_micro_rp2040_bridge C CXX ASM)
set(CMAKE_C_STANDARD 11)
set(CMAKE_CXX_STANDARD 17)

pico_sdk_init()

add_executable(codex_micro_rp2040_bridge
    src/bridge_protocol.c
    src/main.c
    src/usb_descriptors.c
)

target_include_directories(codex_micro_rp2040_bridge PRIVATE
    ${CMAKE_CURRENT_LIST_DIR}/src
)

target_link_libraries(codex_micro_rp2040_bridge PRIVATE
    pico_stdlib
    tinyusb_device
    tinyusb_board
)

# TinyUSB owns the native USB controller. Enabling Pico stdio over USB here
# would create a second, conflicting device stack.
pico_enable_stdio_usb(codex_micro_rp2040_bridge 0)
pico_enable_stdio_uart(codex_micro_rp2040_bridge 0)
pico_add_extra_outputs(codex_micro_rp2040_bridge)
