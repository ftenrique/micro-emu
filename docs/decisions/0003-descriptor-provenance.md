# ADR 0003: no inventar los 59 bytes no publicados del descriptor USB

- Estado: Aceptada
- Fecha: 2026-07-29

## Contexto

FreeMicro publica un descriptor observado de 216 bytes y afirma que el
descriptor USB de su unidad mide 275 bytes. La secuencia pública de 216 bytes
ya contiene Input, Output y Feature para Report ID 6, pero no aporta los 59
bytes USB restantes.

## Decisión

Se conserva byte por byte únicamente la captura publicada de 216 bytes,
marcada como observada por BLE. No se rellena ni se transforma para aparentar
el descriptor USB de 275 bytes.

## Consecuencias

- El núcleo puede verificar Report ID 6 y los informes de 63 bytes.
- Antes del spike VHF definitivo debe capturarse el descriptor USB completo de
  una unidad real o conseguirse una fuente verificable.
- Una prueba VHF con la captura de 216 bytes será explícitamente exploratoria;
  un fallo de reconocimiento no refutará por sí solo la arquitectura.
