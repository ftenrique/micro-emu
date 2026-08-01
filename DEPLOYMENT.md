# Deployment

Este proyecto se despliega como una combinación de firmware para una placa
RP2040 y un puente Rust que se ejecuta en Windows. No hay un servidor web ni
un paquete npm que publicar.

## Requisitos

- Windows PowerShell 5.1 o posterior.
- Node.js 20 o posterior.
- Rust 1.85 o posterior con Cargo.
- Una placa RP2040 Zero y el cable USB.
- Aproximadamente 1,3 GiB libres en `D:` para el toolchain aislado del RP2040.

## Desde un checkout limpio

```powershell
git clone https://github.com/ftenrique/micro-emu.git
Set-Location .\micro-emu
npm test
npm run verify:descriptor
npm run bridge:test
npm run bridge:build
```

Los comandos anteriores validan el núcleo del protocolo y compilan el puente
sin tocar el dispositivo.

## Compilar y verificar el firmware

La primera ejecución instala el toolchain RP2040 dentro de la ubicación
aislada definida por el proyecto:

```powershell
npm run rp2040:setup
npm run rp2040:check
npm run rp2040:build
npm run rp2040:verify
```

`rp2040:verify` comprueba que el artefacto generado corresponde al descriptor
esperado. No continúes si falla alguna de estas comprobaciones.

## Flashear la placa

1. Desconecta la placa RP2040.
2. Mantén pulsado el botón `BOOTSEL` mientras conectas el USB.
3. Ejecuta `npm run rp2040:flash` y sigue las indicaciones del script.
4. Desconecta y vuelve a conectar la placa normalmente.
5. Ejecuta `npm run rp2040:port` para localizar el puerto serie.

El script de flasheo está pensado para una placa RP2040 Zero. Comprueba el
modelo y la unidad detectada antes de confirmar cualquier escritura.

## Ejecutar el puente

Inicia el puente usando el puerto serie devuelto por el paso anterior:

```powershell
npm run bridge:run -- -- --port COM7
```

Reemplaza `COM7` por el puerto real. El puente presenta la interfaz HID+CDC y
transporta los mensajes del protocolo; debe permanecer abierto durante la
sesión de prueba.

## Validación física opcional

Con el software OEM del teclado cerrado, valida el dispositivo AJAZZ con:

```powershell
npm run hardware:test -- --listen 45
```

Este comando escribe seis cuadros en las LCD y lee teclas, encoders y
pulsaciones. Es una prueba de hardware y no debe ejecutarse contra un
dispositivo que no sea el perfil compatible documentado.

## Problemas frecuentes

- **No aparece el puerto:** vuelve a conectar la placa en modo normal y
  ejecuta `npm run rp2040:port` de nuevo.
- **Falla la compilación del firmware:** ejecuta `npm run rp2040:check` y
  repite `npm run rp2040:setup` si falta el toolchain.
- **El teclado no reacciona:** cierra el software OEM y confirma que se usa la
  colección vendor `MI_00 / FFA0:0001`, descrita en
  [docs/hardware-profile.md](docs/hardware-profile.md).
- **El puente no conecta:** verifica el puerto, el cable USB y que ningún otro
  proceso tenga abierta la conexión serie.

## Publicar una nueva versión

Antes de crear un tag, ejecuta todas las comprobaciones locales:

```powershell
npm test
npm run verify:descriptor
npm run bridge:test
npm run bridge:build
npm run rp2040:build
npm run rp2040:verify
git tag v0.1.0
git push origin main --follow-tags
```

Actualiza el número de versión del tag cuando corresponda. Los artefactos de
hardware y los inventarios locales no deben incluirse en el commit salvo que
estén expresamente documentados y sean reproducibles.