# Spike KMDF/VHF

Controlador mínimo para publicar un HID virtual con VID `303A`, PID `8360`,
Usage Page `FF00` y Report ID 6.

El kernel sólo:

- valida y transporta informes de 64 bytes;
- conserva hasta 128 informes escritos por ChatGPT;
- permite a una herramienta de usuario enviar informes de entrada;
- expone contadores diagnósticos.

No interpreta JSON, no ejecuta procesos y no accede a red o filesystem.

## Límite conocido del descriptor

El spike usa la captura de 216 bytes publicada por FreeMicro. El descriptor USB
real se reporta con 275 bytes, pero los 59 restantes no están disponibles. Un
fallo de detección con este spike no permite concluir que VHF sea inviable.

## IOCTL de diagnóstico

| IOCTL | Dirección | Contenido |
|---|---|---|
| `IOCTL_CODEX_GET_OUTPUT_REPORT` | driver → monitor | secuencia + informe normalizado de 64 bytes |
| `IOCTL_CODEX_SEND_INPUT_REPORT` | monitor → driver | informe de 64 bytes, primer byte `0x06` |
| `IOCTL_CODEX_GET_STATS` | driver → monitor | capturados, descartados, enviados e inválidos |

La interfaz de control usa
`{7A50A0E8-289F-4A72-9BC4-11A40FC1A63C}`.
