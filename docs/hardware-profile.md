# Perfil de hardware

La unidad conectada se verificó el 29 de julio de 2026 como **AJAZZ AKP03E
rev. 2**. El perfil reproducible está en
`hardware/profiles/ajazz-akp03e-rev2-0300-3002.verified.json`.

## Identidad e interfaces

| Campo | Valor verificado |
|---|---|
| VID:PID | `0300:3002` |
| Revisión USB | `0x0002` |
| Fabricante/producto | `HOTSPOTEKUSB` / `HOTSPOTEKUSB HID DEMO` |
| Interfaz de control | `MI_00`, Usage Page `FFA0`, Usage `0001` |
| Informes de control | entrada 513 bytes, salida 1025 bytes |
| Interfaz secundaria | `MI_01`, teclado `0001:0006` |
| Controles | 9 teclas y 3 encoders con pulsación |
| Pantallas | 6 LCD |

Los tamaños HID incluyen el byte de Report ID. El protocolo transporta 512
bytes de entrada y 1024 de salida más el prefijo `0x00`.

## Resultado físico

- La interfaz vendor se abre para lectura y escritura desde un proceso de
  usuario.
- Las seis LCD aceptaron un patrón distinto por tecla.
- La fuente segura es RGB 126×126, rotada 90° en sentido horario y codificada
  como JPEG 60×60 antes de enviarla.
- Se capturaron las nueve teclas, giro en ambos sentidos de los tres encoders y
  pulsación/liberación de los tres encoders.
- Las capturas se guardaron en
  `artifacts/akp03e-rev2-explicit-mi00-events.jsonl` y
  `artifacts/akp03e-rev2-lower-buttons-encoder-presses.jsonl`.

## Hallazgo de interoperabilidad

La selección de interfaz es obligatoria. Abrir por VID/PID/serie sin filtrar
puede escoger `MI_01`; Windows acepta las escrituras, pero el firmware las
ignora y no entrega eventos vendor. El probador fija explícitamente
`MI_00 / FFA0:0001`.

Ejecutar una prueba nueva:

```powershell
npm run hardware:test -- --listen 45
```

La prueba escribe seis cuadros numerados y registra eventos JSONL. Debe cerrarse
antes el software OEM para evitar que conserve el handle HID.

El perfil `ajazz-akp03.pending.json` se conserva como plantilla para otras
revisiones. `usb-candidate-04b4-1007.observed.json` corresponde a otro
periférico que permaneció conectado al retirar el AKP03E.
