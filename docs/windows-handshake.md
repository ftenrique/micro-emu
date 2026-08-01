# Prueba de reconocimiento de ChatGPT mediante VHF

> **Ruta histórica, no recomendada.** Esta guía documenta el spike KMDF/VHF
> que se compiló y firmó, pero no se instaló. La ruta activa utiliza el
> RP2040 Zero y está descrita en [rp2040-bridge.md](rp2040-bridge.md); no
> requiere cambiar Secure Boot, activar `testsigning` ni instalar un
> controlador propio.

## Estado previo confirmado

- El AJAZZ conectado es un AKP03E rev. 2 `0300:3002`.
- Su canal vendor `MI_00 / FFA0:0001` se abre desde usuario y quedó validado
  con escritura en las seis LCD y lectura de las nueve teclas y tres encoders.
- La interfaz `04B4:1007` pertenece a otro periférico y queda fuera de esta
  prueba.
- El spike virtual usa VID `303A`, PID `8360`, Usage Page `FF00`, Report ID 6.
- La herramienta de monitor responde automáticamente a `device.status`.

## Toolchain confirmado

Visual Studio 2022, MSBuild x64 y WDK `26100.6584` están disponibles, incluidos
`VhfKm.lib`, `WindowsKernelModeDriver10.0`, `Inf2Cat.exe`, `signtool.exe` y
`devcon.exe`. Comprobar con:

```powershell
.\driver\vhf-spike\scripts\check-toolchain.ps1
```

El paquete Release x64 compila sin advertencias. La build de viabilidad
desactiva Spectre para evitar instalar bibliotecas adicionales; la firma e
instalación continúan requiriendo elevación y confirmación explícita.

## 1. Compilar

```powershell
.\driver\vhf-spike\scripts\build-driver.ps1 -Configuration Release
```

No continuar si hay warnings del descriptor, INF o análisis estático.

## 2. Crear y confiar en el certificado de prueba

Primero crear el certificado sin cambiar almacenes del sistema:

```powershell
$certificate = .\driver\vhf-spike\scripts\create-test-certificate.ps1 |
  ConvertFrom-Json
$certificate
```

Para confiar en ese mismo certificado, ejecutar desde PowerShell elevado:

```powershell
.\driver\vhf-spike\scripts\trust-test-certificate.ps1 `
  -CertificatePath $certificate.certificate `
  -AcknowledgeMachineTrustChange
```

Esto modifica `LocalMachine\Root` y `LocalMachine\TrustedPublisher`. No se
debe repetir `create-test-certificate.ps1`: crearía un certificado nuevo con
otra huella.

## 3. Generar catálogo y firmar

```powershell
.\driver\vhf-spike\scripts\sign-driver.ps1 `
  -PackageDirectory .\driver\vhf-spike\x64\Release\CodexMicroVhf `
  -CertificateThumbprint $certificate.thumbprint
```

`artifacts\driver-signing` contiene el certificado exportado, no el paquete
del controlador. El script firma primero el `.sys`, vuelve a generar el
catálogo con `Inf2Cat` y firma el `.cat` al final.

Antes de instalar la confianza del certificado, `signtool verify` terminará
con “root certificate which is not trusted”. Es el resultado esperado: la
firma y la pertenencia del `.sys` al catálogo ya se pueden comprobar, pero
Windows todavía no confía en el certificado autofirmado.

## 4. Activar firma de prueba

Sólo en la máquina de prueba y desde PowerShell elevado:

```powershell
.\driver\vhf-spike\scripts\enable-test-signing.ps1 `
  -AcknowledgeRebootAndSecurityImpact
Restart-Computer
```

No desactivar Secure Boot sin confirmar antes que se dispone de la clave de
recuperación de BitLocker y una vía de recuperación.

## 5. Instalar el dispositivo raíz

Desde PowerShell elevado:

```powershell
.\driver\vhf-spike\scripts\install-driver.ps1 `
  -InfPath <ruta-a-CodexMicroVhf.inf> `
  -AcknowledgeTestDriverRisk
```

Confirmar en Administrador de dispositivos:

- `Codex Micro VHF Feasibility Spike` bajo dispositivos de sistema;
- un hijo HID con `VID_303A&PID_8360`;
- sin códigos de problema.

## 6. Ejecutar el monitor antes de abrir ChatGPT

```powershell
dotnet run --project .\tools\protocol-monitor\ProtocolMonitor.csproj `
  -c Release -- --serve 120 --capture .\artifacts\chatgpt-handshake.jsonl
```

El log normal sólo muestra método, id y tamaño. `--verbose` imprime JSON
completo y no debe usarse si una futura versión transporta texto de tareas.

## 7. Forzar el descubrimiento

1. Cerrar ChatGPT por completo.
2. Comprobar que el monitor sigue activo.
3. Abrir ChatGPT.
4. Abrir configuración y buscar Codex Micro.
5. Conservar la captura aunque no aparezca la sección.

Se espera observar alguno de:

- `device.status`;
- `v.oai.rgbcfg`;
- `v.oai.thstatus`.

## 8. Emitir una tecla

Con el monitor sirviendo en otra consola:

```powershell
dotnet run --project .\tools\protocol-monitor\ProtocolMonitor.csproj `
  -c Release -- --emit AG00 --stats
```

Repetir con `AG01`. Registrar cualquier cambio visible de tarea.

## 9. Repetibilidad

Repetir:

- tras cerrar y abrir ChatGPT;
- tras deshabilitar/habilitar el dispositivo;
- tras reiniciar Windows.

La prueba sólo pasa si el reconocimiento y `AG00` se reproducen.

## 10. Retirada

Desde PowerShell elevado:

```powershell
.\driver\vhf-spike\scripts\uninstall-driver.ps1 -AcknowledgeRemoval
```

Después de retirar el driver, se puede desactivar test-signing con:

```powershell
bcdedit /set testsigning off
```

y reiniciar.

## Interpretación de un fallo

El descriptor incorporado tiene 216 bytes, mientras que FreeMicro reporta 275
bytes para USB. Si ChatGPT no reconoce el spike, el siguiente experimento debe
usar una captura USB completa antes de descartar VHF. Después se investigarán
cadenas de producto/fabricante, Hardware IDs, Container ID e interfaces
adicionales, en ese orden.
