---
agent: devin-local
session: ubiquitous-feast
created: 2026-08-05T10:33:19Z
---
# Soporte Hermes Desktop Agent en el bridge micro-emu (daemon multi-agente)

Convertir el bridge RP2040 en un daemon único que posee el hardware (controlador físico + RP2040 opcional) y sirve simultáneamente a Codex/ChatGPT (ruta HID actual + MCP) y a Hermes Desktop Agent (solo MCP, sin emulación Codex Micro), mediante proxies STDIO por agente.

## Contexto actual

- `tools/rp2040-bridge` (Rust, deps: `ajazz-sdk`, `hidapi`, `image`, `serde_json`) abre el controlador físico (AJAZZ/Stream Deck) y el puerto CDC del RP2040; el firmware presenta el HID Codex Micro a ChatGPT.
- Modo MCP actual (`--mcp`): STDIO JSON-RPC por líneas; **un solo agente** puede ser dueño del proceso (y del COM/HID) a la vez (<ref_file file="D:\Programming\micro-emu\tools\rp2040-bridge\src\mcp.rs" />, `run_mcp` en <ref_file file="D:\Programming\micro-emu\tools\rp2040-bridge\src\main.rs" />).
- Eventos físicos (`PhysicalEvent` en <ref_file file="D:\Programming\micro-emu\tools\rp2040-bridge\src\codex.rs" />) fluyen solo hacia ChatGPT vía HID (`poll_controller` → `RadialState::event` → `send_codex_message`).
- `v.oai.thstatus` / `v.oai.rgbcfg` llegan desde ChatGPT por HID y pintan los 6 slots LCD (`process_codex_message`).
- Hermes Agent soporta servidores MCP `stdio` en `~/.hermes/config.yaml` (`mcp_servers.<name>.command/args`).

## Decisiones acordadas con el usuario

1. **Transporte:** daemon único + proxies STDIO por agente.
2. **Eventos hacia Hermes:** tool de polling (el bridge bufferiza; Hermes consulta).
3. **Reparto fijo:** teclas `AG00–AG02` + slots LCD 1–3 → Codex/ChatGPT; teclas `AG03–AG05` + slots 4–6 → Hermes. Teclas aux (`ACT06–ACT08`) y rotores permanecen en Codex (ajustable en una fase posterior si hace falta).
4. **Modo standalone:** el daemon debe poder arrancar sin RP2040 (`--port none`), sirviendo solo MCP + controlador físico.

## Arquitectura objetivo

```text
AJAZZ / Stream Deck ──HID── daemon rp2040-bridge ──CDC── RP2040 ──HID── ChatGPT
                                   │ (127.0.0.1:TCP, JSON-RPC por líneas)
                     ┌─────────────┴─────────────┐
             proxy STDIO (--agent codex)   proxy STDIO (--agent hermes)
                     │                            │
                 Codex CLI                Hermes Desktop Agent
```

- El daemon escucha en `127.0.0.1:<puerto>` (solo loopback; por defecto p. ej. `48360`, configurable con `--bind`). Wire format: JSON-RPC delimitado por `\n`, idéntico al STDIO MCP actual → el proxy es un simple bombeo de líneas.
- Cada conexión TCP es una **sesión MCP independiente** (hace su propio `initialize`). El proxy envía como primera línea un saludo interno `{"bridge":"hello","agent":"codex|hermes"}` que el daemon consume para etiquetar la sesión (no se reenvía al cliente MCP).
- Un solo hilo principal sigue siendo dueño del hardware; las sesiones TCP corren en hilos lectores que inyectan las peticiones por `mpsc` con un tag de sesión, y cada sesión tiene su `Sender` de respuesta.

## Pasos de implementación

Todos los cambios viven en `tools/rp2040-bridge/src/` salvo docs y scripts.

### 1. Refactor previo (sin cambio funcional)
- Extraer de `main.rs` la lógica compartida de runtime (`BridgeRuntime`, `open_runtime`, `process_codex_message`, `poll_controller`, `bridge_status`, reconexiones) a un módulo `runtime.rs` para que la usen los tres modos (legacy, mcp-stdio, daemon).
- Hacer opcional el bloque serie en `BridgeRuntime` (`Option<SerialRuntime>` + `firmware/port` opcionales) para el modo standalone. `--port none` salta `open_serial_runtime`, el health-check y la reconexión CDC.

### 2. Enrutado de eventos y partición fija (`routing.rs`)
- Definir `AgentId { Codex, Hermes }` y la partición fija:
  - `Button 0..=2` (→ `AG00–AG02`), aux y rotores → destino Codex.
  - `Button 3..=5` (→ `AG03–AG05`) → destino Hermes.
- En `poll_controller`: los eventos de la mitad Codex siguen yendo al HID (`send_codex_message`) **si hay RP2040**; si no (standalone), se encolan para la sesión MCP `codex` si existe. Los eventos de la mitad Hermes se traducen a un JSON estable (`{"key":"AG03","pressed":true,"ts":...}`) y se encolan en la cola de Hermes (cola acotada, p. ej. 256 eventos, descarta los más antiguos).
- Fusión de LCD: el daemon mantiene el estado combinado de 6 slots.
  - `v.oai.thstatus` recibido por HID (ChatGPT) se **recorta a los slots 1–3** antes de aplicarse.
  - `set_thread_status` desde la sesión Hermes se recorta a los slots 4–6; desde la sesión Codex, a 1–3 (en standalone sin sesión codex, Hermes puede escribir los 6 con un flag `--lcd-full-hermes`, opcional).
  - El replay tras reconexión del controlador usa el estado fusionado (sustituye a `last_thread_status`).

### 3. Daemon (`daemon.rs`) — nueva bandera `--daemon`
- `TcpListener` en `127.0.0.1` (`--bind 127.0.0.1:48360` por defecto). Rechazar binds no-loopback.
- Hilo aceptador + hilo lector por conexión → `mpsc::Sender<(SessionId, Result<Value,String>)>` hacia el bucle principal; mapa `SessionId → (AgentId, escritor TCP)`.
- El bucle principal (adaptación de `run_mcp`) multiplexa: peticiones MCP de N sesiones, eventos serie, poll del controlador, health-check y reconexiones. Las respuestas/errores van solo a la sesión origen.
- `initialize` responde con `serverInfo.name = "micro-emu-bridge"` e instrucciones específicas por agente.
- Sesión duplicada del mismo agente: la nueva reemplaza a la anterior (se cierra la vieja) para soportar reinicios de Codex/Hermes sin reiniciar el daemon.

### 4. Proxy STDIO (`proxy.rs`) — nueva bandera `--mcp-proxy --agent codex|hermes`
- Conecta a `--connect 127.0.0.1:48360` (default igual al daemon), envía el saludo `bridge hello` y bombea líneas stdin→TCP y TCP→stdout.
- Si la conexión falla y se pasa `--autostart`, lanza el daemon desacoplado (`rp2040-bridge --daemon --port auto --controller ajazz`, args heredables vía `--daemon-args`), con lockfile en `%LOCALAPPDATA%` para evitar dobles arranques, y reintenta con backoff.
- Si el daemon cae, el proxy responde a las peticiones con error `-32001 "bridge daemon unavailable"` y reintenta (mismo patrón que `reconnect_mcp` actual).

### 5. Tools MCP por agente (`mcp.rs`)
- Nueva tool `poll_events`:
  - `inputSchema`: `{ "timeout_ms": number (0..=25000, default 0) }`.
  - Devuelve y drena la cola del agente llamante. Con `timeout_ms>0` es long-poll: el bucle principal difiere la respuesta hasta que haya eventos o venza el plazo (máx. una `poll_events` pendiente por sesión; una segunda cancela la primera con resultado vacío).
- Filtrado por agente en `tools/list` y `tools/call`:
  - **codex**: todas las actuales (`bridge_status`, `emit_key`, `send_codex_message`, `set_thread_status` [slots 1–3], `set_rgb_config`, `device_status`) + `poll_events`. Las tools que requieren RP2040 devuelven `tool_error` claro en standalone.
  - **hermes**: `bridge_status`, `poll_events`, `set_thread_status` (slots 4–6), `set_rgb_config`. Sin `emit_key`/`send_codex_message`/`device_status` (son de la emulación Codex Micro).
- Extender `bridge_status` con: `mode` (`daemon`/`mcp`/`legacy`), `rp2040` (present/absent), `agents` conectados, partición y tamaño de colas.

### 6. CLI y compatibilidad (`main.rs`)
- Nuevos flags: `--daemon`, `--bind`, `--mcp-proxy`, `--agent`, `--connect`, `--autostart`, `--port none`.
- `--mcp` (STDIO directo) se conserva intacto para no romper la configuración Codex existente; `--daemon`, `--mcp` y `--legacy` son mutuamente excluyentes; `--mcp-proxy` ignora las opciones de hardware.
- Validaciones: `--agent` obligatorio con `--mcp-proxy`; `--port none` incompatible con `--emit`.

### 7. Scripts y documentación
- `package.json`: añadir `bridge:daemon` (`cargo run --release -- --daemon --port auto`) y `bridge:daemon:standalone` (`--daemon --port none`).
- `README.md`: nueva sección "Integrate with Hermes Desktop Agent" + subsección del daemon compartido:
  - Config Hermes (`~/.hermes/config.yaml`):
    ```yaml
    mcp_servers:
      micro_emu_bridge:
        command: "D:\\Programming\\micro-emu\\tools\\rp2040-bridge\\target\\release\\rp2040-bridge.exe"
        args: ["--mcp-proxy", "--agent", "hermes", "--autostart"]
    ```
  - Config Codex (`.codex/config.toml`): mismo exe con `--mcp-proxy --agent codex --autostart` (o mantener `--mcp` si solo se usa un agente).
  - Documentar el reparto fijo de teclas/slots y el flujo `poll_events`.
- `docs/rp2040-bridge.md`: diagrama y descripción del modo daemon, partición y modo standalone.

### 8. Tests
- Unit tests Rust: partición de eventos (routing), recorte/fusión de `thstatus` por agente, cola acotada, filtrado de tools por agente, parseo de los nuevos flags, framing del saludo del proxy.
- Test de integración ligero: daemon en `--port none --controller none` + dos conexiones TCP simuladas (codex y hermes) haciendo `initialize`, `tools/list`, `set_thread_status` y `poll_events` (inyectando eventos sintéticos).
- Verificar que los tests existentes (`framing`, `messages`, `descriptor`, host-test de firmware) no cambian.

## Archivos a modificar / crear

- `tools/rp2040-bridge/src/main.rs` — CLI, dispatch de modos, refactor.
- `tools/rp2040-bridge/src/runtime.rs` — **nuevo**, runtime compartido (serie opcional).
- `tools/rp2040-bridge/src/routing.rs` — **nuevo**, `AgentId`, partición, colas, fusión LCD.
- `tools/rp2040-bridge/src/daemon.rs` — **nuevo**, listener TCP y bucle multi-sesión.
- `tools/rp2040-bridge/src/proxy.rs` — **nuevo**, proxy STDIO↔TCP con autostart.
- `tools/rp2040-bridge/src/mcp.rs` — tools por agente, `poll_events`, helpers de sesión.
- `package.json` — scripts `bridge:daemon*`.
- `README.md`, `docs/rp2040-bridge.md` — documentación Hermes/daemon.

Sin dependencias nuevas: TCP loopback con `std::net`, hilos con `std::thread` (coherente con el estilo actual del crate).

## Verificación

- [ ] `npm run bridge:test` (cargo test) — unit + integración nuevos y existentes.
- [ ] `npm run bridge:build` compila sin warnings nuevos.
- [ ] `npm test` (protocolo JS) sin cambios.
- [ ] Manual con hardware: `npm run bridge:daemon -- -- --port auto`; registrar proxy en Codex (`codex mcp list` + `bridge_status`) y en Hermes (`/reload-mcp`, pedir tools); pulsar `AG04` y comprobar que Hermes lo recibe con `poll_events`; `set_thread_status` desde Hermes pinta solo slots 4–6 mientras ChatGPT pinta 1–3.
- [ ] Manual standalone: `--daemon --port none --controller ajazz` sin RP2040 conectado.

## Riesgos / consideraciones

- **Long-poll en bucle único:** `poll_events` con timeout exige respuestas diferidas; se implementa con una lista de "polls pendientes" revisada en cada iteración (el bucle ya itera con sleeps de 25 ms).
- **Puerto TCP local:** solo loopback y sin datos sensibles; si se quisiera endurecer, un token en `%LOCALAPPDATA%` puede añadirse después sin cambiar el protocolo.
- **Autostart doble:** mitigado con lockfile + reintento de conexión antes de lanzar.
- **Compatibilidad:** `--mcp` STDIO se mantiene; la configuración Codex actual sigue funcionando sin tocarla.
- La identidad fina de rotores/teclas aux queda en Codex por ahora; hacer la partición configurable es una extensión natural posterior.
