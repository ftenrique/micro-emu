# Puente físico RP2040 Zero

## Arquitectura

```text
AJAZZ AKP03E (0300:3002, FFA0:0001)
        │ HID vendor
        ▼
tools/rp2040-bridge (Rust, espacio de usuario)
        │ USB CDC, protocolo CM v1 + CRC16
        ▼
RP2040 Zero
        │ USB HID 303A:8360, Report ID 6
        ▼
Windows HID nativo → ChatGPT
```

ChatGPT nunca abre el puerto CDC. El firmware presenta primero la interfaz HID
Codex Micro y después el canal CDC usado por el puente.

## Qué está implementado

- Interfaz vendor HID USB de 49 bytes en MI_00 (Report ID 6); keyboard/consumer/mouse quedan en una interfaz separada de 167 bytes.
- Informes HID `Input`, `Output` y `Feature` de 63 bytes con Report ID 6.
- Endpoint HID de entrada y salida.
- Transporte CDC binario con magic `CM`, versión, tipo, secuencia, longitud y
  CRC16-CCITT.
- Reensamblado JSON Codex Micro en el puente, aceptando mensajes con `CRLF` y JSON sin terminador.
- Ruta rápida de `device.status` en el firmware para aislar el handshake HID.
- Respuesta a `device.status`.
- Teclas LCD 1-6 → `AG00`-`AG05`.
- Teclas inferiores → `ACT06`-`ACT08`.
- Los dos rotores laterales generan `v.oai.rad` como cruceta (izquierdo: izquierda/derecha; derecho: arriba/abajo). Cada sentido acumula dos niveles: primer detent `d=0.5`, segundo detent `d=1.0`; al invertir el sentido se reinicia en `d=0.5`. Al pulsarlos, el izquierdo genera `ACT12` (enviar chat, botón fijo final) y el derecho conserva `ACT10`. El rotor central conserva `ENC_CW` (`act: 2`), `ENC_CC` (`act: 2`) y `ENC_CLK` (`act: 1/0`).
- `v.oai.thstatus` → seis cuadros numerados con color y brillo en el AJAZZ. Un estado `e:0` (OFF) o `b:0` borra por completo el slot; los slots inactivos permanecen negros.
- `v.oai.rgbcfg` se acepta y confirma, pero no repinta las LCD; el fondo permanece negro y las ranuras solo cambian con `v.oai.thstatus`. El bridge inicia la retroiluminación LCD al 100%, usa `CLE/FF` + `STP` para purgar las LCD y después escribe una imagen JPEG negra explícita (CLE por slot al liberar un botón), porque CLE solo deja el gris neutro del firmware.
- Las llamadas RPC con `id` de `v.oai.rgbcfg` y `v.oai.thstatus` reciben
  un ACK correlacionado por HID (`{"result":true,"id":...}`) con `CRLF`;
  las notificaciones sin `id` no reciben respuesta.
- Apertura explícita del AJAZZ por Usage Page `FFA0`, Usage `0001`.

La identidad de encoder no existe en los mensajes públicos: los tres encoders
se reducen por ahora al control genérico de Codex Micro.

## Herramientas necesarias

El toolchain queda fijado por `tools/rp2040-toolchain.lock.json`:

- Pico SDK 2.3.0 y TinyUSB en los commits indicados;
- Arm GNU Toolchain 14.2.Rel1, verificado por SHA-256;
- fuente de picotool 2.3.0;
- CMake, Ninja y MSVC ya presentes en Visual Studio.

La instalación es local a `.toolchains/`, ignorada por Git y situada en `D:`.
No modifica el PATH del sistema:

```powershell
npm run rp2040:setup
```

Comprobar:

```powershell
npm run rp2040:check
```

El script acepta una ruta explícita:

```powershell
.\tools\check-rp2040-toolchain.ps1 `
  -PicoSdkPath D:\tools\pico-sdk `
  -ArmToolchainPath D:\tools\arm-gnu-toolchain
```

El setup descarga únicamente el SDK, `lib/tinyusb`, picotool y el compilador.
No inicializa los submódulos de red, Bluetooth o TLS. El ZIP del compilador se
elimina después de verificarlo y extraerlo.

## Construcción

Con `PICO_SDK_PATH` definido:

```powershell
npm run descriptor:generate:rp2040
npm test
npm run bridge:test
npm run bridge:build
npm run rp2040:build
npm run rp2040:verify
```

El resultado esperado es:

```text
firmware\rp2040-zero\build\codex_micro_rp2040_bridge.uf2
```

La build usa por defecto `PICO_BOARD=waveshare_rp2040_zero`, definición incluida
en Pico SDK para la placa de 2 MiB con el WS2812 en GPIO 16 que clona el modelo
comprado. Si la placa recibida requiere una definición distinta se puede pasar:

```powershell
.\tools\build-rp2040.ps1 -Board <nombre-pico-sdk>
```

`-Board pico` queda como alternativa compatible si el clon no acepta la
definición específica; este firmware no usa el LED ni los GPIO.

## Flasheo recuperable

1. Desconectar la RP2040 Zero.
2. Mantener pulsado `BOOT`.
3. Conectar el USB sin soltar `BOOT`.
4. Soltar cuando aparezca la unidad `RPI-RP2`.
5. Ejecutar:

```powershell
npm run rp2040:flash
```

El reinicio tras la copia es automático. Este proceso reemplaza sólo el
firmware de la placa; no modifica Windows. El script valida la cabecera UF2 y
exige que haya exactamente una unidad `RPI-RP2` antes de copiar.

## Primera prueba

Localizar el puerto `COM` llamado `micro-emu bridge` y cerrar el software AJAZZ
oficial:

```powershell
npm run rp2040:port
```

Después:

```powershell
npm run bridge:run -- -- --port COM7
```

npm 11 consume `--port` y `--listen` como configuración propia, así que hace
falta un segundo `--`. Alternativa sin npm:

```powershell
.\tools\rp2040-bridge\target\release\rp2040-bridge.exe --port COM7
```

Antes de abrir ChatGPT deben aparecer:

```json
{"type":"bridge-ready","firmware":"rp2040-zero/0.1.1-diag","ajazzConnected":true}
```

Las seis pantallas mostrarán cuadros numerados. Abrir entonces ChatGPT y
comprobar:

1. aparición de Codex Micro;
2. `codex-report` con cabecera HID y prefijo de payload;
3. solicitud `device.status`;
4. `device.status resp queued`, `hid in accepted` y `hid in complete`;
5. `codex-response` con `result:true` para `v.oai.rgbcfg`/`v.oai.thstatus`;
6. cambio de color mediante `v.oai.thstatus`;
7. recepción de `AG00` y `AG01`.

Para probar únicamente firmware y ChatGPT sin abrir el AJAZZ:

```powershell
npm run bridge:run -- -- `
  --port COM7 `
  --no-ajazz `
  --listen 120 `
  --emit AG00 `
  --emit-after 10
```

Este comando espera diez segundos y emite una pulsación y liberación `AG00`.
Así se puede separar el reconocimiento de ChatGPT de cualquier problema del
AJAZZ.

## Límites de la primera versión

- El descriptor USB completo de 275 bytes sigue pendiente de captura.
- No se emulan todavía cadenas o descriptores adicionales no publicados.
- El protocolo CDC no transporta prompts ni cuerpos de tareas en los logs.
- Una desconexión finaliza el proceso; la reconexión automática se añadirá
  después de superar la puerta de reconocimiento de ChatGPT.


## Ejecución estable

El proceso permanece activo indefinidamente si se omite `--listen`; `--listen N` queda reservado para pruebas y `--listen 0` equivale también a ilimitado.
