# Alcance de las fases 0 y 1

## Objetivo

Preparar una base reproducible para decidir si la emulación es viable,
incluida la comprobación física del AJAZZ y un dispositivo USB RP2040, sin
instalar código propio en kernel.

## Incluido

- Inventario de Windows, ChatGPT, WDK y dispositivos PnP candidatos.
- Perfil verificado del AKP03E rev. 2.
- Contrato de 63 bytes del canal Codex Micro.
- Framing USB y BLE para conservar paridad con la fuente.
- Reensamblado tolerante a errores con límites de memoria.
- Mensajes `device.status`, `v.oai.hid`, `v.oai.rad`, `v.oai.rgbcfg` y
  `v.oai.thstatus`.
- Respuesta segura a métodos desconocidos.
- Descriptor HID observado y verificador estructural.
- Fixtures y pruebas sin hardware.
- Escritura de las seis LCD y lectura de todos los controles mediante
  `MI_00 / FFA0:0001`.
- Firmware RP2040 HID+CDC y protocolo de transporte con CRC.
- Puente Rust AJAZZ↔RP2040 con pruebas sin hardware.

## No incluido

- Instalación del controlador KMDF/VHF.
- Cambios de Secure Boot o de la política de arranque.
- Puente activo con ChatGPT.
- Métodos destructivos o de firmware.

## Criterio de decisión

Las fases implementadas demuestran que el AJAZZ puede actuar como entrada y
salida física del puente, y reducen el riesgo del protocolo Codex Micro. El
reconocimiento por ChatGPT sólo se declara viable después de superar la puerta
USB física de Fase 2 con el RP2040.

## Supuestos pendientes

- La versión objetivo de ChatGPT conserva el descubrimiento de Codex Micro.
- El descriptor que usa el descubrimiento de Windows coincide con el observado.
