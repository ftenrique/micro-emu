# Núcleo del protocolo Codex Micro

Biblioteca ESM sin I/O ni dependencias de plataforma.

## Contrato de framing

| Transporte | Buffer entregado | Bytes |
|---|---|---:|
| USB | `[0x02][len][data...][padding]` | 63 |
| BLE | `[0x06][0x02][len][data...][padding]` | 64 |

`len` puede valer de 1 a 61. El mensaje lógico es UTF-8, termina en `\r\n` y
puede cruzar informes. BLE consume un byte adicional para el Report ID, por lo
que conserva los mismos 61 bytes de datos por fragmento.

## Decodificación segura

`FrameDecoder.feed()` nunca lanza por datos del cable. Devuelve:

```js
{
  messages: [{ /* objeto JSON completo */ }],
  errors: [ProtocolError]
}
```

Rechaza longitudes y cabeceras incorrectas, aísla JSON/UTF-8 inválido, limita el
buffer acumulado a 64 KiB y permite continuar con el informe siguiente.
`finish()` convierte cualquier resto sin CRLF en `TRUNCATED_MESSAGE`.

## Descriptor

La captura publicada contiene 216 bytes y define:

- Report ID 1: teclado.
- Report ID 2: consumer control.
- Report ID 3: ratón.
- Report ID 6, Usage Page `0xFF00`: Input, Output y Feature de 63 bytes.

FreeMicro informa de 275 bytes para USB, pero no publica en la secuencia
observada los 59 bytes restantes. La metadata mantiene `usbDescriptorComplete:
false` para impedir que el spike confunda una aproximación con identidad exacta.

## Métodos modelados

- `device.status`
- `v.oai.hid`
- `v.oai.rad`
- `v.oai.rgbcfg`
- `v.oai.thstatus`

No se modelan operaciones de bootloader ni escritura/borrado de filesystem.
