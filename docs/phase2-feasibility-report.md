# Informe de viabilidad de Fase 2

Fecha de actualización: 30 de julio de 2026.

## Resultado actual y cambio de arquitectura

La preparación técnica de Fase 2 está **aprobada**:

- el AKP03E físico funciona en lectura y escritura;
- el protocolo portable y el monitor pasan sus pruebas;
- el driver KMDF/VHF compila para x64;
- el paquete SYS/INF/CAT supera `Inf2Cat`;
- el preflight VHF histórico llegó a devolver `readyForEndToEndTest: true`.

El paquete llegó a firmarse localmente, pero no se instaló y no se modificó el
arranque de Windows. Tras revisar el impacto de Secure Boot, la ruta principal
se trasladó a un dispositivo USB físico RP2040. El controlador VHF queda como
evidencia técnica, no como siguiente paso recomendado.

El preflight actual usa la arquitectura RP2040. Las comprobaciones de
repositorio pasan y `readyToBuildRp2040Firmware` es `true`, pero
`readyForEndToEndTest` permanece en `false` hasta flashear y conectar la placa.

El 30 de julio se instaló de forma local el toolchain fijado, se compiló el
firmware Release para `waveshare_rp2040_zero` y picotool validó la imagen:

- Pico SDK 2.3.0;
- Arm GNU Toolchain 14.2.Rel1;
- UF2 de 43.520 bytes, 85 bloques;
- descriptor Codex Micro de 216 bytes presente en la imagen compilada;
- SHA-256 de la build:
  `3b7a2dbdca2fd6e81beea032b12819b0e5561c9b50cc4432804f3bad48627adf`.

## Evidencia positiva

- AJAZZ AKP03E rev. 2 `0300:3002`, canal `MI_00 / FFA0:0001`.
- Seis LCD, nueve teclas, tres encoders y sus pulsaciones verificados.
- 30 pruebas del protocolo Codex Micro aprobadas.
- Descriptor de 216 bytes sincronizado entre JavaScript y el driver.
- `ajazz-doctor` y `protocol-monitor` compilan; el monitor supera
  fragmentación, `device.status` y `AG00`.
- WDK `26100.6584`: `VhfKm.lib`, toolset x64, `Inf2Cat`, `signtool` y `devcon`
  presentes.
- Release x64: cero advertencias y cero errores; catálogo generado sin errores
  de signability.

## Condición de la build

La build de viabilidad fuerza `SpectreMitigation=false` para evitar instalar
las bibliotecas Spectre adicionales de MSVC en una máquina con espacio
limitado. Es apropiada para este experimento local, pero antes de convertir el
spike en un controlador de producción debe recompilarse con mitigación Spectre.

El empaquetado y la firma están separados deliberadamente. `driver:build`
produce artefactos sin firmar; los scripts de certificado y firma requieren una
decisión explícita.

## Siguiente puerta

1. Flashear `codex_micro_rp2040_bridge.uf2`.
2. Ejecutar el puente Rust sobre el puerto CDC.
3. Abrir ChatGPT y conservar la captura estructurada.
4. Probar reconocimiento y `device.status`.
5. Emitir `AG00` sintético y después probar `AG00`/`AG01` desde el AJAZZ.

Estos pasos no afectan certificados, arranque ni almacén de controladores.
