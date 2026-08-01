# Preparación del WDK para el spike VHF

> **Ruta histórica.** El WDK ya no es necesario para la solución RP2040.
> Esta página se conserva únicamente para reproducir el spike KMDF/VHF. La
> ruta recomendada está en [rp2040-bridge.md](rp2040-bridge.md).

El proyecto fija `WindowsTargetPlatformVersion` en `10.0.26100.0`. Microsoft
publica WDK `26100.6584` como el kit soportado para Visual Studio 2022; el
número base del SDK y del WDK debe coincidir.

## Acción manual necesaria

Esta preparación modifica Visual Studio y requiere UAC:

1. Abrir Visual Studio Installer y actualizar Visual Studio 2022 Community.
2. Seleccionar **Modificar > Componentes individuales > Windows Driver Kit**.
3. Instalar el SDK y WDK `26100.6584` desde
   [Other WDK downloads](https://learn.microsoft.com/windows-hardware/drivers/other-wdk-downloads).
4. No aceptar un reinicio automático; cerrar trabajo abierto y reiniciar de
   forma controlada si el instalador lo solicita.

La sesión automatizada no puede confirmar el escritorio seguro de UAC. No se
ha instalado ni modificado el arranque de Windows desde este repositorio.

## Verificación

Después de completar el instalador:

```powershell
npm run driver:check
npm run driver:build
```

El primer comando debe mostrar:

- `readyToBuild: True`;
- `requiredKitVersion: 10.0.26100.0`;
- rutas no nulas para `VhfKm.lib`, `Inf2Cat.exe` y el platform toolset.

La firma de prueba, confianza del certificado, `testsigning` e instalación del
driver son pasos separados y deliberadamente requieren confirmaciones
explícitas. Véase `docs/windows-handshake.md`.
