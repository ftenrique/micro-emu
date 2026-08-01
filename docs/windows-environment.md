# Entorno Windows observado

Captura local realizada el 29 de julio de 2026.

| Campo | Valor observado |
|---|---|
| Sistema | Windows 10 Pro 22H2 |
| Build | 19045.5011 |
| Arquitectura | AMD64 |
| Paquete ChatGPT | Directorio de paquete presente; versión no accesible desde la sesión |
| Paquete Codex | Directorio de paquete presente |
| AKP03 presente | AJAZZ AKP03E rev. 2, `0300:3002` |
| Canal de control | `MI_00`, `FFA0:0001`, entrada 513/salida 1025 bytes |
| Codex Micro `303A:8360` | No detectado todavía |
| Node.js | 24.0.1 |
| Rust | 1.95.0 |
| .NET SDK | 8.0.203 |
| Cabeceras `vhf.h` | Incluida `10.0.26100.0` |
| `VhfKm.lib` | Disponible para x64 |
| MSVC/MSBuild | Visual Studio 2022 Community 17.14.37, MSBuild x64 |
| `signtool.exe` | Disponible para `10.0.26100.0` |
| `Inf2Cat` / `devcon` | Disponibles |

## Consecuencias

- La ruta física AKP03E quedó validada de extremo a extremo desde usuario:
  enumeración, escritura de las seis LCD y lectura de todos los controles.
- El equipo no satisface el objetivo declarado de Windows 11, aunque las fases
  portables y la prueba física funcionan en Windows 10.
- `Get-PnpDevice` puede devolver acceso denegado; el inventario usa `pnputil`
  como alternativa para localizar la familia `0300:*`.
- El WDK `26100.6584` está integrado y el paquete Release x64 se genera sin
  advertencias.
- La build de viabilidad usa `SpectreMitigation=false` para no instalar las
  bibliotecas Spectre adicionales de MSVC.

La herramienta `tools/inventory-windows.ps1` debe repetirse en la máquina y
sesión exactas donde se vaya a probar ChatGPT.
