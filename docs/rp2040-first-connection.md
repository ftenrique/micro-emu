# Primera conexión del RP2040 Zero

## Desde un release (sin compilar)

Si descargaste el bundle de Windows (`micro-emu-vX.Y.Z-windows-x64.zip`), no
necesitas compilar el firmware: el bundle incluye el UF2 precompilado
(`codex_micro_rp2040_bridge.uf2`) y un asistente de flasheo.

1. Cerrar ChatGPT y el software oficial de AJAZZ.
2. Mantener pulsado `BOOT`, conectar un cable USB de datos y soltar `BOOT` al
   aparecer `RPI-RP2`.
3. Doble clic en `Flash-Firmware.cmd` dentro de la carpeta extraída.

La placa se reinicia sola y `RPI-RP2` desaparece. Reinicia el puente (o Codex
Micro) para que detecte la placa. También se publica el UF2 como un asset
independiente del release para reflashear sin volver a descargar el bundle.

El resto del documento describe el flujo de desarrollo, que compila el firmware
desde fuente.

## Antes de conectarlo

```powershell
npm run rp2040:check
npm run rp2040:verify
```

Ambos comandos deben terminar correctamente. El UF2 preparado está en:

```text
firmware\rp2040-zero\build\codex_micro_rp2040_bridge.uf2
```

## Flasheo

1. Cerrar ChatGPT y el software oficial de AJAZZ.
2. Desconectar el RP2040.
3. Mantener pulsado `BOOT`.
4. Conectar un cable USB de datos y soltar `BOOT` al aparecer `RPI-RP2`.
5. Ejecutar `npm run rp2040:flash`.
6. Esperar cinco segundos al reinicio automático.

No es necesario instalar un controlador ni cambiar Secure Boot. Si algo sale
mal, repetir BOOT siempre vuelve a exponer `RPI-RP2`.

## Comprobación de Windows

```powershell
npm run rp2040:port
npm run inventory
```

El primer comando debe encontrar `micro-emu bridge` y un puerto `COMx`. El
inventario debe encontrar al menos una colección `VID_303A&PID_8360`.

Si no aparece:

1. cambiar el cable por uno confirmado para datos;
2. probar otro puerto USB sin hub;
3. comprobar si `RPI-RP2` sigue montado, lo que indicaría que no se copió el
   UF2;
4. repetir el flasheo.

## Prueba aislada de ChatGPT

Abrir ChatGPT y ejecutar, sustituyendo `COM7`:

```powershell
npm run bridge:run -- -- `
  --port COM7 `
  --no-ajazz `
   `
  --emit AG00 `
  --emit-after 10
```

El segundo `--` es obligatorio: npm 11 consume `--port` y `--listen` como
configuración propia y sólo reenvía lo que aparece tras el segundo separador.
Llamar al binario directamente evita el problema:

```powershell
.\tools\rp2040-bridge\target\release\rp2040-bridge.exe --port COM7 
```

Resultados esperados:

1. log `bridge-ready` con firmware `rp2040-zero/0.1.1-diag`;
2. aparición de Codex Micro en ChatGPT;
3. trazas `codex-report` y recepción de `device.status`;
4. logs `device.status req`, `device.status resp queued` y `hid in complete`;
5. `codex-response` con `result:true` para `v.oai.rgbcfg`/`v.oai.thstatus`;
6. log `synthetic-event` para `AG00`;
7. una reacción visible de ChatGPT.


## Prueba completa con AJAZZ

Conectar el AKP03E, cerrar su software OEM y ejecutar:

```powershell
npm run bridge:run -- -- --port COM7 
```

Las seis LCD deben mostrar cuadros numerados. Probar primero las teclas 1 y 2,
y después un encoder. Conservar la salida completa de la consola si ChatGPT no
reconoce el dispositivo: permitirá distinguir enumeración USB, protocolo HID y
mapeo físico.
