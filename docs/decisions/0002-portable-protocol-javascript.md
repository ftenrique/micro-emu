# ADR 0002: núcleo portable en JavaScript ESM

- Estado: Aceptada
- Fecha: 2026-07-29

## Decisión

El núcleo del protocolo se implementa como módulos JavaScript ESM sin
dependencias, probado con el runner integrado de Node.js.

## Motivo

- Se ejecuta en Windows, macOS y Linux.
- Permite validar bytes exactos sin WDK ni hardware.
- El spike VHF puede mantener el kernel como transporte y delegar JSON a una
  herramienta de usuario.
- Evita descargar paquetes durante la prueba de viabilidad.

## Consecuencias

El controlador de Fase 2 seguirá siendo C/KMDF. No se comparte parsing de JSON
con el kernel; el límite de confianza permanece explícito.
