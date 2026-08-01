# Plan de implementación: emulación de Codex Micro con AJAZZ AKP03

> **Actualización 30-07-2026:** la ruta principal de Fase 2 se ha trasladado
> de KMDF/VHF a un dispositivo USB físico RP2040 Zero. El firmware publica el
> HID y el puente Windows usa una interfaz CDC nativa. Esto evita instalar
> código propio en kernel, desactivar Secure Boot o activar `testsigning`.
> El desarrollo VHF se conserva como spike histórico. La arquitectura y los
> comandos vigentes están en `docs/rp2040-bridge.md` y ADR 0004.

## 1. Objetivo

Crear una integración local para Windows que permita utilizar un AJAZZ AKP03 con la aplicación de escritorio de ChatGPT como si fuera un Codex Micro.

La solución deberá:

- Presentar ante Windows un dispositivo HID físico compatible con Codex Micro.
- Conseguir que ChatGPT de escritorio detecte ese dispositivo.
- Traducir las pulsaciones y encoders del AJAZZ a eventos Codex Micro.
- Traducir los estados enviados por ChatGPT a imágenes o colores en las seis teclas LCD del AJAZZ.
- Funcionar directamente con la aplicación de escritorio, sin MCP ni Codex App Server.
- Recuperarse de desconexiones, suspensión, cierre de procesos y reinicios.
- Mantener toda la lógica compleja fuera del kernel.

## 2. Arquitectura propuesta

```text
┌──────────────────┐      USB HID       ┌────────────────────────┐
│   AJAZZ AKP03    │ <────────────────> │ Puente Rust (usuario)  │
│ teclas/pantallas │                    └───────────┬────────────┘
└──────────────────┘                                │ USB CDC
                                                    ▼
                                         ┌──────────────────────┐
                                         │ RP2040 Zero          │
                                         │ HID Codex Micro + CDC│
                                         └──────────┬───────────┘
                                                    │ USB HID
                                                    ▼
                                         ┌──────────────────────┐
                                         │ ChatGPT de escritorio│
                                         └──────────────────────┘
```

### Responsabilidad de cada componente

#### Adaptador AJAZZ

- Detectar la revisión exacta del AKP03.
- Leer teclas, botones y encoders.
- Actualizar las seis pantallas LCD.
- Administrar reconexiones y conflictos con el software OEM.

#### Puente Rust

- Reensamblar y decodificar el protocolo Codex Micro.
- Traducir estados de ChatGPT a imágenes y colores.
- Traducir entradas del AJAZZ a eventos Codex Micro.
- Implementar gestos, temporizaciones y configuración.
- Mantener logs y diagnósticos.

#### Firmware RP2040

- Publicar físicamente el descriptor HID esperado.
- Exponer la identidad USB de Codex Micro mediante TinyUSB.
- Transportar informes entre ChatGPT y el puente por CDC.
- Validar tamaños y tipos de informe.
- No interpretar JSON ni ejecutar acciones.

## 3. Base técnica disponible

El proyecto [FreeMicro](https://github.com/eliBenven/freemicro) proporciona:

- VID `0x303A`.
- PID `0x8360`.
- Usage Page `0xFF00`.
- Report ID `6`.
- Descriptor HID del dispositivo real.
- Framing de informes HID.
- Protocolo JSON-RPC.
- Eventos `v.oai.hid` y `v.oai.rad`.
- Métodos `v.oai.rgbcfg` y `v.oai.thstatus`.
- Comportamiento de `device.status`.
- Colores y temporizaciones de fábrica.
- Identificadores de teclas y encoder.

La implementación deberá reutilizar únicamente los elementos compatibles con su licencia MIT y mantener la atribución correspondiente.

## 4. Alcance del MVP

El primer resultado funcional incluirá:

- Detección del dispositivo virtual por ChatGPT.
- Seis teclas LCD asociadas a seis tareas.
- Estados:
  - Inactiva.
  - Trabajando.
  - Finalizada y no leída.
  - Requiere intervención.
  - Error.
  - Sin tarea asignada.
- Selección de tarea mediante pulsación.
- Doble pulsación para traer ChatGPT al frente, si la aplicación responde al gesto oficial.
- Acciones:
  - Aprobar.
  - Rechazar.
  - Enviar.
  - Nueva tarea o bifurcar tarea.
  - Modo rápido.
- Encoder:
  - Giro horario.
  - Giro antihorario.
  - Pulsación.
- Reconexión automática.
- Herramienta de diagnóstico.

### Fuera del MVP

- Distribución pública con firma comercial del controlador.
- Compatibilidad con todas las revisiones del AKP03.
- Emulación completa de batería real.
- Bluetooth virtual.
- Escritura en el filesystem interno del Codex Micro.
- Métodos de bootloader o actualización de firmware.
- Paridad física exacta con el joystick analógico original.

---

# Fases de ejecución

## Fase 0 — Inventario y contrato del proyecto

### Propósito

Fijar el hardware, el sistema objetivo y los límites antes de desarrollar el controlador.

### Trabajo

1. Registrar el modelo exacto:
   - AKP03.
   - AKP03E.
   - AKP03R.
   - Otra revisión o dispositivo remarcado.
2. Obtener:
   - VID/PID.
   - Firmware.
   - Interfaces HID.
   - Tamaños de informe.
   - Transporte utilizado.
3. Registrar:
   - Versión de Windows.
   - Versión de ChatGPT de escritorio.
   - Arquitectura del sistema.
4. Verificar que un proceso propio puede abrir el AJAZZ con el software OEM cerrado.
5. Decidir si el primer resultado será:
   - Prototipo personal con firma de prueba.
   - Producto distribuible con firma de producción.
6. Crear el repositorio y el registro de decisiones.

### Entregables

- `docs/scope.md`
- `docs/hardware-profile.md`
- `docs/decisions/`
- `README.md`
- Perfil JSON de la revisión del AJAZZ.

### Criterios de salida

- El AJAZZ se enumera de forma reproducible.
- Se pueden leer sus entradas.
- Se puede actualizar al menos una pantalla.
- La versión inicial de ChatGPT queda registrada.
- El alcance del MVP queda aceptado.

### Decisión

Si el dispositivo no puede leerse y escribirse desde un proceso propio, se detendrá el camino software hasta encontrar una biblioteca compatible o evaluar un proxy físico.

---

## Fase 1 — Núcleo del protocolo Codex Micro

### Propósito

Crear una biblioteca portable y cubierta por pruebas a partir de los hallazgos de FreeMicro.

### Trabajo

1. Incorporar con atribución:
   - Descriptor HID.
   - Constantes de protocolo.
   - Framing de mensajes.
   - Decodificador de tramas.
2. Implementar:
   - Report ID 6.
   - Informes de 63 bytes.
   - Mensajes JSON terminados en CRLF.
   - Fragmentación de mensajes largos.
   - Reensamblado de mensajes fragmentados.
3. Modelar:
   - `device.status`.
   - `v.oai.hid`.
   - `v.oai.rad`.
   - `v.oai.rgbcfg`.
   - `v.oai.thstatus`.
4. Crear respuestas seguras para métodos desconocidos.
5. Generar fixtures basados en mensajes conocidos.
6. Añadir pruebas de:
   - Framing.
   - Fragmentación.
   - Concatenación.
   - Datos truncados.
   - JSON inválido.
   - Longitudes incorrectas.

### Entregables

- `protocol/`
- `protocol/descriptors/`
- `tests/protocol/`
- `NOTICE`
- Fixtures de entrada y salida.

### Criterios de salida

- Las tramas conocidas producen exactamente los bytes esperados.
- Los mensajes fragmentados se reconstruyen sin pérdidas.
- Los informes incorrectos se rechazan sin bloquear el proceso.
- La biblioteca no contiene dependencias de macOS ni del AJAZZ.

---

## Fase 2 — Spike de reconocimiento en ChatGPT

> Esta es la puerta crítica del proyecto.

### Propósito

Demostrar lo antes posible que la aplicación de escritorio para Windows acepta
un dispositivo HID físico RP2040 con la identidad documentada.

### Trabajo

1. Crear un firmware TinyUSB mínimo para RP2040 Zero.
2. Configurar:
   - VID `0x303A`.
   - PID `0x8360`.
   - Usage Page `0xFF00`.
   - Report ID `6`.
   - Descriptor HID observado.
3. Añadir CDC al dispositivo compuesto como canal privado con el puente Rust.
4. Registrar todos los informes enviados por ChatGPT.
5. Responder a `device.status`.
6. Comprobar si:
   - Aparece la sección Codex Micro en la configuración.
   - ChatGPT envía `v.oai.rgbcfg`.
   - ChatGPT envía `v.oai.thstatus`.
   - ChatGPT consulta `device.status`.
7. Emitir eventos `AG00` y `AG01`.
8. Verificar si ChatGPT cambia de tarea.
9. Repetir la prueba después de reiniciar Windows y ChatGPT.

### Entregables

- `firmware/rp2040-zero/`
- `tools/rp2040-bridge/`
- `docs/rp2040-bridge.md`
- Captura de la secuencia de conexión.
- Informe de compatibilidad.

### Criterios de salida

- ChatGPT detecta el HID físico RP2040.
- La sección de Codex Micro aparece en la aplicación.
- Se reciben mensajes de configuración o iluminación.
- Un evento `AG00` produce una acción visible.
- El reconocimiento funciona de forma repetible.

### Decisión

#### Si funciona

Continuar con el adaptador AJAZZ.

#### Si no funciona

Investigar, en este orden:

1. Cadenas de fabricante y producto.
2. Hardware IDs y Container ID.
3. Descriptor de configuración completo.
4. Diferencias en la ruta de descubrimiento para Windows.
5. Otras interfaces HID del dispositivo original.
6. Emulación mediante USB gadget físico.

No se desarrollará el puente completo antes de resolver esta fase.

---

## Fase 3 — Adaptador físico para AKP03

### Propósito

Encapsular todas las particularidades del AJAZZ detrás de una interfaz estable.

### Trabajo

1. Seleccionar la base adecuada:
   - `mirajazz`.
   - `soomfon`.
   - HID directo.
2. Detectar el dispositivo por perfil y no por suposiciones globales.
3. Leer:
   - Seis teclas LCD.
   - Botones adicionales.
   - Pulsaciones de encoders.
   - Giro horario y antihorario.
4. Actualizar:
   - Imagen individual.
   - Color sólido.
   - Brillo, si está soportado.
   - Estado apagado.
5. Implementar:
   - Reconexión.
   - Cancelación.
   - Cierre limpio.
   - Detección de conflicto con el software OEM.
6. Crear un modo `dry-run`.

### Interfaz prevista

```text
IAjazzDevice
├── enumerate()
├── open()
├── close()
├── read_event()
├── set_key_image(index, image)
├── set_key_color(index, color)
├── clear_key(index)
├── set_brightness(value)
└── get_device_info()
```

### Entregables

- `service/adapters/ajazz/`
- `hardware/profiles/`
- `tools/ajazz-doctor/`
- Mapa físico de entradas.

### Criterios de salida

- Todas las entradas se identifican inequívocamente.
- Cada pantalla puede actualizarse de forma individual.
- No se pierden giros del encoder durante escrituras de imagen.
- La desconexión y reconexión se recuperan automáticamente.
- El software OEM no puede competir silenciosamente por el dispositivo.

---

## Fase 4 — Servicio traductor extremo a extremo

### Propósito

Unir el HID físico RP2040 y el AJAZZ manteniendo toda la lógica de negocio en
espacio de usuario.

### Trabajo

1. Definir un protocolo CDC local versionado.
2. Limitar el firmware a:
   - Validar el Report ID.
   - Validar longitudes.
   - Transportar informes.
3. Reensamblar JSON-RPC en el servicio.
4. Traducir mensajes de ChatGPT a operaciones AJAZZ.
5. Traducir eventos AJAZZ a `v.oai.hid`.
6. Enviar eventos al HID mediante TinyUSB.
7. Responder `device.status` con:
   - Versión simulada.
   - Perfil activo.
   - Batería configurable.
   - Estado de carga.
8. Serializar y deduplicar escrituras.
9. Aplicar límites de frecuencia.
10. Añadir trazas estructuradas.

### Flujo de salida

```text
ChatGPT
  -> informe HID de salida
  -> firmware RP2040
  -> USB CDC
  -> decodificador JSON-RPC
  -> mapa de estados
  -> imágenes/colores AJAZZ
```

### Flujo de entrada

```text
AJAZZ
  -> evento de tecla o encoder
  -> adaptador de hardware
  -> mapa Codex Micro
  -> v.oai.hid
  -> USB CDC
  -> informe HID RP2040
  -> ChatGPT
```

### Entregables

- `firmware/rp2040-zero/`
- `tools/rp2040-bridge/`
- `docs/rp2040-bridge.md`
- Herramienta de monitorización.

### Criterios de salida

- ChatGPT muestra estados distintos en las seis teclas.
- Una tecla AJAZZ selecciona la tarea correspondiente.
- Reiniciar el servicio recupera el canal sin reinstalar el controlador.
- El flujo no utiliza MCP ni App Server.
- El kernel no analiza JSON.

---

## Fase 5 — Paridad de interacción

### Propósito

Reproducir el comportamiento visible del Codex Micro utilizando los controles disponibles en el AKP03.

### Mapa de estados

| Estado Codex | Representación propuesta |
|---|---|
| Sin tarea | Pantalla apagada |
| Inactiva | Fondo blanco o icono neutro |
| Trabajando | Azul |
| Finalizada/no leída | Verde |
| Requiere intervención | Ámbar |
| Error | Rojo |
| Seleccionada | Pulso, borde o icono adicional |

### Trabajo

1. Mapear `AG00`–`AG05` a las seis teclas LCD.
2. Implementar pulsación simple.
3. Implementar doble pulsación dentro de 350 ms.
4. Mapear botones a:
   - Fast.
   - Aprobar.
   - Rechazar.
   - Dividir o nueva tarea.
   - Voz.
   - Enviar.
5. Reproducir:
   - `ENC_CW`.
   - `ENC_CC`.
   - `ENC_CLK`.
6. Evitar aceleración artificial del encoder.
7. Implementar apagado automático y restauración.
8. Limpiar el estado verde cuando la tarea pasa a estar leída.
9. Crear asignaciones alternativas para controles ausentes.

### Entregables

- `service/mapping/`
- `assets/key-icons/`
- `tests/acceptance/gesture-matrix.*`
- Configuración predeterminada.

### Criterios de salida

- Cada estado tiene una prueba automatizada.
- Cada gesto tiene una prueba automatizada.
- Aprobar y rechazar generan exactamente una acción.
- La doble pulsación no se activa entre teclas distintas.
- El encoder no duplica eventos.
- Las seis pantallas siguen siendo legibles con brillo reducido.

---

## Fase 6 — Seguridad, fallos y recuperación

### Propósito

Evitar que un periférico, un informe corrupto o un proceso caído afecten a la estabilidad del sistema o autoricen acciones inesperadas.

### Reglas de seguridad

- El controlador no interpretará JSON.
- El controlador no ejecutará procesos.
- El controlador no accederá a red.
- No se implementará `sys.bootloader`.
- No se implementará `fs.write`.
- No se implementará `fs.delete`.
- No se escribirán prompts, código ni secretos en los logs.
- Los eventos de aprobación deberán conservar semántica explícita.

### Trabajo

1. Validar:
   - Tamaño de informes.
   - Report ID.
   - Longitudes internas.
   - Frecuencia.
   - Dirección de cada mensaje.
2. Añadir:
   - Watchdog.
   - Backoff de reconexión.
   - Timeouts.
   - Cierre limpio.
   - Apagado de pantallas al salir.
3. Detectar procesos que compiten por el AJAZZ.
4. Fuzzear:
   - Informes HID.
   - Fragmentación.
   - JSON.
   - IPC.
5. Probar:
   - Desconexión durante una escritura.
   - Caída del servicio.
   - Caída de ChatGPT.
   - Suspensión.
   - Hibernación.
   - Reinicio.
6. Revisar el límite de confianza kernel/usuario.

### Entregables

- `security/threat-model.md`
- `tests/fuzz/`
- `tests/reliability/`
- Política de métodos permitidos.
- Procedimientos de recuperación.

### Criterios de salida

- No se producen bloqueos ni pantallazos con entradas corruptas.
- El servicio recupera hot-plug y suspensión.
- La ausencia del AJAZZ no impide abrir ChatGPT.
- La caída del servicio no deja acciones repetidas.
- Los logs no contienen contenido de las tareas.

---

## Fase 7 — Instalación y operación en Windows

### Propósito

Convertir el prototipo en una instalación reversible y diagnosticable.

### Trabajo

1. Elegir:
   - Firma de prueba para uso personal.
   - Firma de producción para distribución.
2. Crear instalador para:
   - Controlador.
   - Servicio.
   - Perfil AJAZZ.
   - Recursos gráficos.
3. Configurar inicio automático.
4. Implementar actualización con rollback.
5. Crear herramientas:
   - `doctor`.
   - `status`.
   - `logs`.
   - `diagnostic-export`.
6. Implementar desinstalación completa.
7. Documentar recuperación tras una actualización incompatible de ChatGPT.

### Entregables

- `installer/`
- `tools/doctor/`
- `docs/install.md`
- `docs/uninstall.md`
- Plan de firma.
- Plan de actualización.

### Criterios de salida

- Instalación y desinstalación funcionan en una máquina limpia.
- No quedan servicios, controladores ni dispositivos huérfanos.
- Una actualización fallida vuelve a la versión anterior.
- El usuario puede saber qué proceso posee el AJAZZ.
- El diagnóstico identifica ausencia, conflicto y fallo de protocolo.

---

## Fase 8 — Beta, compatibilidad y mantenimiento

### Propósito

Demostrar que el resultado se mantiene estable ante variaciones reales de hardware y actualizaciones de la aplicación.

### Trabajo

1. Ejecutar una matriz sobre las versiones objetivo de Windows 11.
2. Registrar las versiones de ChatGPT verificadas.
3. Validar otras revisiones del AKP03 cuando haya hardware disponible.
4. Mantener fixtures por versión.
5. Detectar cambios incompatibles de protocolo.
6. Añadir diagnósticos locales y opcionales.
7. Documentar limitaciones conocidas.
8. Preparar una versión candidata.

### Entregables

- `docs/compatibility.md`
- `docs/release-checklist.md`
- `CHANGELOG.md`
- Informe beta.
- Matriz de compatibilidad.
- Versión candidata.

### Criterios de salida

- Todas las pruebas globales tienen evidencia reproducible.
- Un cambio incompatible genera un diagnóstico claro.
- La versión puede instalarse, actualizarse y retirarse.
- Sólo se declaran compatibles las versiones realmente verificadas.

---

# 5. Estrategia de pruebas

## 5.1 Pruebas unitarias

- Framing HID.
- Reensamblado.
- JSON-RPC.
- Mapeo de estados.
- Gestos temporizados.
- Conversión de imágenes.
- Deduplicación.
- Límites de frecuencia.

## 5.2 Pruebas de contrato

- Biblioteca de protocolo ↔ fixtures FreeMicro.
- Puente ↔ firmware.
- Servicio ↔ adaptador AJAZZ.
- Descriptor HID ↔ expectativas de Windows.

## 5.3 Pruebas de integración

- ChatGPT ↔ HID físico RP2040.
- RP2040 ↔ puente por CDC.
- Servicio ↔ AJAZZ.
- Flujo completo bidireccional.

## 5.4 Pruebas físicas

- Pulsación simple.
- Pulsación larga.
- Doble pulsación.
- Giro rápido del encoder.
- Varias teclas consecutivas.
- Escritura de todas las pantallas.
- Desenchufar durante una actualización.
- Reconectar en otro puerto.

## 5.5 Pruebas de recuperación

- Reinicio del servicio.
- Cierre forzado del servicio.
- Cierre de ChatGPT.
- Suspensión.
- Hibernación.
- Reinicio de Windows.
- Conflicto con el software OEM.

# 6. Criterios globales de terminado

## Integración

- ChatGPT de escritorio detecta el dispositivo.
- La configuración de Codex Micro está disponible.
- Las seis tareas muestran su estado en el AJAZZ.
- Las pulsaciones controlan tareas reales de la aplicación.

## Seguridad

- El kernel sólo transporta informes validados.
- No existen métodos destructivos.
- Los logs no contienen información de las tareas.
- Los eventos corruptos no provocan fallos del sistema.

## Fiabilidad

- Hot-plug y suspensión se recuperan automáticamente.
- El servicio puede reiniciarse sin reinstalar.
- No se duplican aprobaciones ni rechazos.
- Las pantallas no quedan permanentemente en un estado incorrecto.

## Compatibilidad

- La revisión exacta del AJAZZ queda identificada.
- Las variantes se aíslan mediante perfiles.
- Las versiones de ChatGPT compatibles quedan documentadas.

## Operación

- La instalación es reproducible.
- La desinstalación es completa.
- Existe diagnóstico local.
- Las actualizaciones admiten rollback.

# 7. Orden recomendado de ejecución en Codex

1. Crear el repositorio y completar la fase 0.
2. Implementar la biblioteca de protocolo.
3. Construir inmediatamente el spike VHF.
4. Detener el proyecto si ChatGPT no reconoce el dispositivo.
5. En paralelo con la investigación de reconocimiento, crear el adaptador AJAZZ.
6. Unir ambas partes sólo después de superar la puerta crítica.
7. Implementar primero selección y estados.
8. Añadir después aprobación, rechazo, voz y encoder.
9. Completar las pruebas de recuperación antes del instalador.
10. Separar el MVP personal del trabajo de firma y distribución.

# 8. Estructura inicial sugerida

```text
codex-micro-ajazz/
├── README.md
├── NOTICE
├── protocol/
│   ├── descriptors/
│   ├── framing/
│   └── messages/
├── driver/
│   ├── vhf-spike/
│   └── virtual-codex-micro/
├── service/
│   ├── bridge/
│   ├── mapping/
│   └── adapters/
│       └── ajazz/
├── hardware/
│   └── profiles/
├── assets/
│   └── key-icons/
├── tools/
│   ├── protocol-monitor/
│   ├── ajazz-doctor/
│   └── doctor/
├── installer/
├── security/
├── tests/
│   ├── protocol/
│   ├── contract/
│   ├── integration/
│   ├── acceptance/
│   ├── fuzz/
│   └── reliability/
└── docs/
    ├── scope.md
    ├── hardware-profile.md
    ├── windows-handshake.md
    ├── ipc.md
    ├── compatibility.md
    ├── install.md
    ├── uninstall.md
    ├── release-checklist.md
    └── decisions/
```

# 9. Próximo paso

Las fases 0 y 1 están implementadas y el AJAZZ ya está verificado. El siguiente
trabajo práctico es compilar y flashear el RP2040 Zero para responder:

> ¿La versión Windows de ChatGPT reconoce el HID físico `303A:8360`, Usage Page
> `FF00`, Report ID `6`, publicado por el RP2040 con el descriptor observado?

El resultado determina si el descriptor público de 216 bytes es suficiente o
si será necesario capturar los 275 bytes completos del dispositivo original.
