# Codex Micro para AJAZZ AKP03

Prototipo local para comprobar si un AJAZZ AKP03 puede actuar como interfaz
física de Codex Micro ante ChatGPT para Windows mediante un dispositivo USB
RP2040.

El repositorio implementa las fases 0 y 1 y deja materializada la primera
prueba de Fase 2:

- **Fase 0:** contrato, inventario reproducible de Windows y perfil de hardware.
- **Fase 1:** núcleo portable del protocolo, descriptor observado, modelos de
  mensajes, fixtures, pruebas y probador físico AKP03E.
- **Fase 2:** firmware RP2040 HID+CDC y puente Rust en espacio de usuario. El
  spike KMDF/VHF se conserva como investigación, pero ya no es la ruta
  recomendada.

La ruta RP2040 no instala controladores propios, no necesita `testsigning` y no
modifica Secure Boot.

## Requisitos

- Windows PowerShell 5.1 para el inventario.
- Node.js 20 o posterior para la biblioteca y las pruebas.
- Rust 1.85 o posterior para el probador físico y el puente.
- Aproximadamente 1,3 GiB libres en `D:` para el toolchain RP2040 aislado.

No hay dependencias npm.

## Uso rápido

```powershell
npm test
npm run verify:descriptor
npm run inventory
npm run preflight
npm run doctor:build
npm run doctor
npm run hardware:test -- --listen 45
npm run bridge:test
npm run bridge:build
npm run rp2040:setup
npm run rp2040:check
npm run rp2040:build
npm run rp2040:verify
```

Los comandos `driver:*` y `monitor:*` se conservan sólo para reproducir la
ruta VHF histórica.

Para guardar un inventario que se pueda adjuntar a un informe:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass `
  -File .\tools\inventory-windows.ps1 `
  -OutputPath .\artifacts\inventory.json
```

El inventario es de solo lectura: no abre interfaces HID, no instala
controladores y no escribe en el dispositivo. `hardware:test` sí escribe seis
cuadros numerados en las LCD y captura los controles; debe ejecutarse con el
software OEM cerrado.

## Deployment

El procedimiento completo para preparar el puente Rust, compilar y verificar
el firmware RP2040, flashearlo y ponerlo en marcha está en
[DEPLOYMENT.md](DEPLOYMENT.md).

Resumen mínimo:

```powershell
npm test
npm run bridge:build
npm run rp2040:setup
npm run rp2040:build
npm run rp2040:verify
npm run rp2040:flash
npm run rp2040:port
npm run bridge:run -- --port COM7
```

Sustituye `COM7` por el puerto que indique `npm run rp2040:port`. El primer
flasheo requiere conectar la placa RP2040 en modo BOOTSEL; el puente no es un
servicio de producción y debe ejecutarse en el equipo que tenga conectada la
placa.
## API del protocolo

```js
import {
  FrameDecoder,
  frameJson,
  createRequest,
  keyEvent,
} from "./protocol/index.js";

const reports = frameJson(createRequest("device.status", undefined, 1));

const decoder = new FrameDecoder();
for (const report of reports) {
  const { messages, errors } = decoder.feed(report);
  // Procesar mensajes y registrar errores sin detener el proceso.
}

const press = keyEvent("AG00", 1, 0);
```

Cada informe USB mide 63 bytes. Sus dos primeros bytes son opcode `0x02` y
longitud; hasta 61 bytes de datos siguen a la cabecera. El mensaje lógico
termina en CRLF y puede ocupar varios informes.

## Estado de viabilidad

La ruta física está **verificada** con un AJAZZ AKP03E rev. 2 `0300:3002`:
escritura de las seis LCD y lectura de las nueve teclas, tres encoders y sus
pulsaciones. El probador abre explícitamente la colección vendor
`MI_00 / FFA0:0001`; abrir la interfaz de teclado `MI_01` produce falsos éxitos
de escritura sin reacción del firmware.

La viabilidad extremo a extremo con ChatGPT sigue **pendiente** hasta que:

1. llegue la placa RP2040 Zero y se flashee el firmware;
2. ChatGPT reconozca el HID físico y reaccione a `AG00`.

Véanse [docs/hardware-profile.md](docs/hardware-profile.md),
[docs/rp2040-bridge.md](docs/rp2040-bridge.md) y
[docs/windows-environment.md](docs/windows-environment.md).
El estado y los bloqueos observados están resumidos en
[docs/phase2-feasibility-report.md](docs/phase2-feasibility-report.md).

## Seguridad

El núcleo no accede a hardware, red o filesystem. No implementa llamadas a
`sys.bootloader`, `fs.write`, `fs.writebin` ni `fs.delete`.

## Licencia y atribución

Código del proyecto bajo MIT. Las fuentes y condiciones de FreeMicro constan
en [NOTICE](NOTICE).
