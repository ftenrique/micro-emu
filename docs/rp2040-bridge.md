# Puente fÃ­sico RP2040 Zero

## Arquitectura

```text
AJAZZ AKP03E (0300:3002, FFA0:0001)
        â”‚ HID vendor
        â–¼
tools/rp2040-bridge (Rust, espacio de usuario)
        â”‚ USB CDC, protocolo CM v1 + CRC16
        â–¼
RP2040 Zero
        â”‚ USB HID 303A:8360, Report ID 6
        â–¼
Windows HID nativo â†’ ChatGPT
```

ChatGPT nunca abre el puerto CDC. El firmware presenta primero la interfaz HID
Codex Micro y despuÃ©s el canal CDC usado por el puente.

### Controladores físicos

El bridge conserva AJAZZ como controlador predeterminado y admite selección explícita:

```powershell
npm run bridge:run -- -- --port COM7 --controller streamdeck-plus
npm run bridge:run -- -- --port COM7 --controller streamdeck-plus-xl
npm run bridge:run -- -- --port COM7 --controller streamdeck-xl
```

Stream Deck + usa `0FD9:0084`, Stream Deck + XL usa `0FD9:00C6` (36 teclas, 6 rotores y ventana de 1200x100), y el XL original usa `0FD9:006C`. Ambos perfiles XL se abren mediante HID directo; la aplicación oficial de Stream Deck debe estar cerrada. `--controller-serial` resuelve la ambigüedad cuando hay varios dispositivos iguales. `--controller none` y `--no-ajazz` desactivan el controlador físico.

El mapeo mantiene el contrato Codex existente: teclas 0-5 a `AG00`-`AG05`, teclas auxiliares a `ACT06`-`ACT08`, y los tres primeros rotores del Plus/+ XL a los eventos radiales/encoder ya soportados. En el XL original, que no tiene rotores, el bridge reserva las teclas sobrantes salvo una disposición fija: índices 11/18/19/20/27 forman una cruceta (arriba/izquierda/enviar/derecha/abajo), 29/30/31 simulan el rotor (CC/click/CW) y 14 es el botón Mic (`ACT10`). Las teclas reservadas permanecen negras y no producen eventos. Los iconos Mic y Enviar a Codex se dibujan directamente en el XL; las imágenes de estado 0-5 siguen viniendo de `v.oai.thstatus`. No se añaden mensajes Codex nuevos.

### Panel de contexto en Stream Deck +

El servidor MCP existente expone set_display_context; no se añade un segundo
servidor MCP ni se modifica el protocolo Codex Micro. La herramienta acepta
project, task, model, effort, status y progress (0-100). El bridge guarda el
último contexto y lo vuelve a pintar cuando el Stream Deck se reconecta. La
franja táctil permanece inerte en esta versión.

Ejemplo de argumentos:

~~~json
{
  "project": "micro-emu",
  "task": "Stream Deck dashboard",
  "model": "gpt-5",
  "effort": "high",
  "status": "working",
  "progress": 65
}
~~~

Solo se muestran metadatos enviados explícitamente por el cliente MCP; el
bridge no obtiene ni registra el prompt o el cuerpo de la tarea.
# QuÃ© estÃ¡ implementado

- Interfaz vendor HID USB de 49 bytes en MI_00 (Report ID 6); keyboard/consumer/mouse quedan en una interfaz separada de 167 bytes.
- Informes HID `Input`, `Output` y `Feature` de 63 bytes con Report ID 6.
- Endpoint HID de entrada y salida.
- Transporte CDC binario con magic `CM`, versiÃ³n, tipo, secuencia, longitud y
  CRC16-CCITT.
- Reensamblado JSON Codex Micro en el puente, aceptando mensajes con `CRLF` y JSON sin terminador.
- Ruta rÃ¡pida de `device.status` en el firmware para aislar el handshake HID.
- Respuesta a `device.status`.
- Teclas LCD 1-6 â†’ `AG00`-`AG05`.
- Teclas inferiores â†’ `ACT06`-`ACT08`.
- Los dos rotores laterales generan `v.oai.rad` como cruceta (izquierdo: izquierda/derecha; derecho: arriba/abajo). Cada sentido acumula dos niveles: primer detent `d=0.5`, segundo detent `d=1.0`; al invertir el sentido se reinicia en `d=0.5`. Al pulsarlos, el izquierdo genera `ACT12` (enviar chat, botÃ³n fijo final) y el derecho conserva `ACT10`. El rotor central conserva `ENC_CW` (`act: 2`), `ENC_CC` (`act: 2`) y `ENC_CLK` (`act: 1/0`).
- `v.oai.thstatus` â†’ seis cuadros numerados con color y brillo en el AJAZZ. Un estado `e:0` (OFF) o `b:0` borra por completo el slot; los slots inactivos permanecen negros.
- `v.oai.rgbcfg` se acepta y confirma, pero no repinta las LCD; el fondo permanece negro y las ranuras solo cambian con `v.oai.thstatus`. El bridge inicia la retroiluminaciÃ³n LCD al 100%, usa `CLE/FF` + `STP` para purgar las LCD y despuÃ©s escribe una imagen JPEG negra explÃ­cita (CLE por slot al liberar un botÃ³n), porque CLE solo deja el gris neutro del firmware.
- Las llamadas RPC con `id` de `v.oai.rgbcfg` y `v.oai.thstatus` reciben
  un ACK correlacionado por HID (`{"result":true,"id":...}`) con `CRLF`;
  las notificaciones sin `id` no reciben respuesta.
- Apertura explÃ­cita del AJAZZ por Usage Page `FFA0`, Usage `0001`.

La identidad de encoder no existe en los mensajes pÃºblicos: los tres encoders
se reducen por ahora al control genÃ©rico de Codex Micro.

## Herramientas necesarias

El toolchain queda fijado por `tools/rp2040-toolchain.lock.json`:

- Pico SDK 2.3.0 y TinyUSB en los commits indicados;
- Arm GNU Toolchain 14.2.Rel1, verificado por SHA-256;
- fuente de picotool 2.3.0;
- CMake, Ninja y MSVC ya presentes en Visual Studio.

La instalaciÃ³n es local a `.toolchains/`, ignorada por Git y situada en `D:`.
No modifica el PATH del sistema:

```powershell
npm run rp2040:setup
```

Comprobar:

```powershell
npm run rp2040:check
```

El script acepta una ruta explÃ­cita:

```powershell
.\tools\check-rp2040-toolchain.ps1 `
  -PicoSdkPath D:\tools\pico-sdk `
  -ArmToolchainPath D:\tools\arm-gnu-toolchain
```

El setup descarga Ãºnicamente el SDK, `lib/tinyusb`, picotool y el compilador.
No inicializa los submÃ³dulos de red, Bluetooth o TLS. El ZIP del compilador se
elimina despuÃ©s de verificarlo y extraerlo.

## ConstrucciÃ³n

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

La build usa por defecto `PICO_BOARD=waveshare_rp2040_zero`, definiciÃ³n incluida
en Pico SDK para la placa de 2 MiB con el WS2812 en GPIO 16 que clona el modelo
comprado. Si la placa recibida requiere una definiciÃ³n distinta se puede pasar:

```powershell
.\tools\build-rp2040.ps1 -Board <nombre-pico-sdk>
```

`-Board pico` queda como alternativa compatible si el clon no acepta la
definiciÃ³n especÃ­fica; este firmware no usa el LED ni los GPIO.

## Flasheo recuperable

1. Desconectar la RP2040 Zero.
2. Mantener pulsado `BOOT`.
3. Conectar el USB sin soltar `BOOT`.
4. Soltar cuando aparezca la unidad `RPI-RP2`.
5. Ejecutar:

```powershell
npm run rp2040:flash
```

El reinicio tras la copia es automÃ¡tico. Este proceso reemplaza sÃ³lo el
firmware de la placa; no modifica Windows. El script valida la cabecera UF2 y
exige que haya exactamente una unidad `RPI-RP2` antes de copiar.

## Primera prueba

Localizar el puerto `COM` llamado `micro-emu bridge` y cerrar el software AJAZZ
oficial:

```powershell
npm run rp2040:port
```

DespuÃ©s:

```powershell
npm run bridge:run -- -- --port COM7
```

npm 11 consume `--port` y `--listen` como configuraciÃ³n propia, asÃ­ que hace
falta un segundo `--`. Alternativa sin npm:

```powershell
.\tools\rp2040-bridge\target\release\rp2040-bridge.exe --port COM7
```

Antes de abrir ChatGPT deben aparecer:

```json
{"type":"bridge-ready","firmware":"rp2040-zero/0.1.1-diag","ajazzConnected":true}
```

Las seis pantallas mostrarÃ¡n cuadros numerados. Abrir entonces ChatGPT y
comprobar:

1. apariciÃ³n de Codex Micro;
2. `codex-report` con cabecera HID y prefijo de payload;
3. solicitud `device.status`;
4. `device.status resp queued`, `hid in accepted` y `hid in complete`;
5. `codex-response` con `result:true` para `v.oai.rgbcfg`/`v.oai.thstatus`;
6. cambio de color mediante `v.oai.thstatus`;
7. recepciÃ³n de `AG00` y `AG01`.

Para probar Ãºnicamente firmware y ChatGPT sin abrir el AJAZZ:

```powershell
npm run bridge:run -- -- `
  --port COM7 `
  --no-ajazz `
  --listen 120 `
  --emit AG00 `
  --emit-after 10
```

Este comando espera diez segundos y emite una pulsaciÃ³n y liberaciÃ³n `AG00`.
AsÃ­ se puede separar el reconocimiento de ChatGPT de cualquier problema del
AJAZZ.

## LÃ­mites de la primera versiÃ³n

- El descriptor USB completo de 275 bytes sigue pendiente de captura.
- No se emulan todavÃ­a cadenas o descriptores adicionales no publicados.
- El protocolo CDC no transporta prompts ni cuerpos de tareas en los logs.
- El modo MCP acepta port auto, detecta el CDC presente por VID/PID y reintenta
  el handshake si el puerto se desconecta o cambia de nÃƒÂºmero.
- La sesiÃƒÂ³n STDIO de MCP permanece abierta durante la reconexiÃƒÂ³n; las llamadas
  recibidas mientras el hardware vuelve a estar disponible reciben un error
  transitorio y Codex puede reintentarlas.


## EjecuciÃ³n estable

El proceso permanece activo indefinidamente si se omite `--listen`; `--listen N` queda reservado para pruebas y `--listen 0` equivale tambiÃ©n a ilimitado.


## Daemon multi-agente (Codex + ZCode + Hermes)

El bridge puede correr como **daemon** (`--daemon`) que posee el hardware una
vez y sirve a hasta tres agentes simultaneamente por TCP loopback. Cada agente
(Codex CLI, ZCode ADE, Hermes Desktop Agent) lanza un **proxy STDIO**
(`--mcp-proxy`) que se conecta al daemon y bombea lineas JSON-RPC en ambas
direcciones.

``text
AJAZZ / Stream Deck --HID-- daemon --CDC-- RP2040 --HID-- ChatGPT
                                | (127.0.0.1:48360)
                    +-----------+-----------+
            proxy (codex)           proxy (hermes)
                    |                       |
                Codex CLI           Hermes Desktop Agent
``

### Particion dinamica de teclas y slots LCD

La particion de las 6 teclas LCD y 6 slots se ajusta dinamicamente segun
cuantos agentes estan activos. Prioridad: **Codex > ZCode > Hermes**.

- **1 agente activo**: posee las 6 teclas y los 6 slots.
- **2 agentes activos**: el de mayor prioridad toma `AG00`-`AG02` / slots 1-3;
  el otro toma `AG03`-`AG05` / slots 4-6.
- **3 agentes activos**: particion por **columnas**: Codex `AG00`+`AG03`
  (slots 1,4), ZCode `AG01`+`AG04` (slots 2,5), Hermes `AG02`+`AG05`
  (slots 3,6).

"Activo" significa: tiene una sesion MCP viva en el daemon, o (solo Codex)
el RP2040 esta conectado. Cuando el conjunto activo cambia, el daemon espera
750 ms (debounce) y reparticiona. Cada agente activo recibe un evento de
particion via `poll_events`. El estado LCD se conserva durante los cambios.

Ver `docs/ZCode_integration.md` para la matriz completa de particiones.

### Arranque

``powershell
npm run bridge:daemon -- -- --port auto
``

Modo standalone sin RP2040 (solo controlador fisico + MCP):

``powershell
npm run bridge:daemon:standalone
``

### Registro de proxies

**Hermes** (`~/.hermes/config.yaml`):

``yaml
mcp_servers:
  micro_emu_bridge:
    command: "D:\Programming\micro-emu\tools\rp2040-bridge\target\release\rp2040-bridge.exe"
    args: ["--mcp-proxy", "--agent", "hermes", "--autostart"]
``

**Codex** (`.codex/config.toml`):

``toml
[mcp_servers.micro_emu_bridge]
command = "D:\Programming\micro-emu\tools\rp2040-bridge\target\release\rp2040-bridge.exe"
args = ["--mcp-proxy", "--agent", "codex", "--autostart"]
cwd = "D:\Programming\micro-emu"
``

`--autostart` hace que el proxy lance el daemon automaticamente si no esta
corriendo. Un lockfile en `%LOCALAPPDATA%\micro-emu\bridge-daemon.lock`
evita que varios proxies intenten arrancar el daemon simultaneamente. El
daemon solo escucha en `127.0.0.1:48360` (configurable con `--bind`).

### Tools por agente

Hermes ve un conjunto filtrado de tools:

- `bridge_status` - estado del daemon, firmware, controlador y agentes.
- `poll_events` - drena las pulsaciones fisicas bufferizadas para sus teclas.
  Con `timeout_ms > 0` espera hasta ese numero de milisegundos. Tambien
  entrega eventos de cambio de particion.
- `set_thread_status` - actualiza los slots LCD asignados a Hermes.
- `set_rgb_config` - envia `v.oai.rgbcfg`.

ZCode ve las mismas tools que Hermes mas `set_display_context` (panel de
contexto del Stream Deck +).

Codex conserva todas las tools existentes mas `poll_events`.

### Modo `--port none` (standalone)

El daemon puede arrancar sin RP2040 conectado. En ese modo no abre puerto
serie, no hace health-check ni reconexion CDC, y las tools que requieren el
RP2040 (`emit_key`, `send_codex_message`, `device_status`) devuelven un
error claro. El controlador fisico y `poll_events` siguen funcionando.
## Task board and multi-device daemon

Daemon mode pools task slots from all configured HID controllers. Use repeatable `--device KIND[,serial=SERIAL][,task-slots=N]` options; the old single `--controller`/`--controller-serial` flags remain compatibility aliases. AJAZZ has six slots, Stream Deck+ eight, and XL models default to eight. Same-model devices must be selected by serial when ambiguous.

Call `publish_tasks` with snapshot semantics and stable `task_id` values. The return value gives each task's current physical assignment or `null` on overflow. `set_thread_status` adapts legacy six-entry arrays to stable session-local cards. `poll_events` adds `task_selected` and `layout_changed` events while retaining legacy key events for adapter clients. Selected-task context is device-scoped; daemon RGB is centrally managed.

TCP sessions are registered before hello, require a valid hello before tools are exposed, and may share a client kind. Hello metadata includes a generated instance id and focus capability. Disconnects retain cards for 30 seconds for stable-id republish/reclaim. `bridge_status` version 2 exposes sessions, devices, assignments, overflow, per-device selection, queue depth, and leases.
LCD task tiles now show a compact owning-agent label (`codex`, `zcode`, or `hermes`) above a smaller slot number. The color tile and task assignment behavior are unchanged; blank/unassigned tiles remain black.

## Stream Deck plugin controller

The daemon accepts a second session role alongside agent MCP sessions: a **controller hello** with `role: "controller"` and `controller: "streamdeck-plugin"`. When a plugin session connects, the daemon creates a `PluginController` (a virtual `PhysicalController` backed by the session's TCP channels) and registers it as an aux controller in the `BridgeRuntime`. Inbound plugin lines (`{"type":"event","kind":"button|encoder-turn|encoder-button",...}`) are translated to `PhysicalEvent`s; outbound render lines (`{"type":"render","render":"threadStatus|displayContext|rgbConfig",...}`) push state to the plugin for key image and touch-strip updates.

The plugin reports its task-slot capacity with `{"type":"capacity","taskSlots":N}` (clamped to 64). On disconnect, the daemon detaches the controller and removes it from the task board. The plugin can autostart the daemon process when the TCP connect fails (mirrors the proxy `--autostart` path).

This mode is mutually exclusive with the direct-HID `--controller streamdeck-*` backends per device: the plugin requires the official Stream Deck app to be running (it owns HID), while the direct-HID mode requires it to be closed.