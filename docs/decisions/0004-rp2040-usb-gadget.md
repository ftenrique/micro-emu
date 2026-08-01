# ADR 0004: RP2040 como dispositivo USB físico

## Estado

Aceptada para la siguiente prueba de viabilidad.

## Contexto

VHF permite publicar el HID requerido, pero el controlador fuente sólo funciona
en kernel y una firma local exige desactivar Secure Boot para habilitar
`testsigning`. Esa intervención no es apropiada para el ordenador principal
del experimento.

Las APIs de inyección de entrada de Windows no publican un dispositivo PnP con
VID/PID, descriptor e informes vendor arbitrarios. ViGEmBus tampoco sirve:
publica mandos concretos, no el HID vendor de Codex Micro.

## Decisión

La ruta principal será una placa RP2040 Zero conectada por USB:

- interfaz HID 0: identidad `303A:8360` y descriptor Codex Micro observado;
- interfaces CDC 1/2: canal privado entre el firmware y el puente Rust;
- Windows carga sus controladores HID y CDC incluidos;
- el puente abre el AJAZZ por `MI_00 / FFA0:0001`, traduce sus controles y
  actualiza las seis LCD;
- el firmware sólo transporta informes de 64 bytes y no interpreta JSON.

El spike KMDF/VHF se conserva como referencia, pero deja de ser el camino
recomendado.

## Consecuencias

- No se instala código propio en kernel.
- Secure Boot y la política de arranque permanecen intactos.
- El firmware se puede recuperar siempre mediante el botón BOOT y un UF2.
- Hace falta una placa física adicional y un cable USB de datos.
- La prueba sigue limitada por el descriptor público de 216 bytes: la unidad
  USB observada por FreeMicro declara 275 bytes.
- Las cadenas `Work Louder` / `Codex Micro` son una hipótesis de compatibilidad,
  no una captura verificada.
- Debe verificarse que ChatGPT acepte una configuración USB compuesta que añade
  CDC después de la interfaz HID.

## Distribución

La identidad USB replicada se usa exclusivamente para interoperabilidad en un
experimento local. No debe publicarse hardware comercial con un VID/PID ajeno.
