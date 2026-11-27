# RX 6600: auditoría y desarrollo verificado

## Alcance y referencias

Base inspeccionada: `rx6600-optimization`, `3ab0af4`. Referencia AMD:
`4eb6cfdf9e61cb112f1f811344ec0dd309eddd6c`; original:
`63bcca43c6a00a758510398fa43096ce7e4f7df0`. El fork experimental
contiene siete commits sobre la referencia AMD. Entre el original y esa
referencia hay 626 commits y 243 archivos diferentes; esto impide atribuir
todo el comportamiento a las siete optimizaciones del fork.

Se inspeccionaron arquitectura, manifiestos, historial y rutas de caché,
planificación PSO, memoria, pools, comandos, presentación, texturas y media.
Esta es una auditoría inicial de código, no una certificación exhaustiva ni
una validación de las fases A–G. No se han ejecutado activos de juegos.
La licencia y las atribuciones existentes se conservan.

Referencias públicas consultadas:

- https://github.com/wine-mirror/wine/blob/master/README.md
- https://github.com/ValveSoftware/Proton/blob/proton_10.0/README.md
- https://github.com/doitsujin/dxvk/blob/master/README.md
- https://github.com/HansKristian-Work/vkd3d-proton/blob/master/README.md

Wine separa cargador e implementación de API; VIctory ya aplica ese principio
con `victory-loader`, `victory-hle` y ejecución nativa x86-64. DXVK documenta
caché de shaders, compilación anticipada y monitorización del compilador;
la precarga debe competir con trabajo urgente sin perder pases. VKD3D-Proton
traduce D3D12 a Vulkan para su plataforma: no demuestra una ventaja de añadir
Vulkan a un destino Windows D3D12. No se añade otra API gráfica. Estas
conclusiones se basan en documentación pública, no en benchmarks comparativos.

## Mapa técnico y riesgos pendientes

| Área | Evidencia en código | Estado / validación necesaria |
| --- | --- | --- |
| CPU nativa | `victory-loader`, importaciones HLE | Arquitectura adecuada; no se ejecutó un título |
| Shaders | GFX10 → SSA (`ir`), EXEC/control (`structure`), raíces (`resource`), DXIL (`emit`) | Pruebas portables útiles; no equivalen a ejecución DXIL |
| Wave/float | `emit/lower.rs`, `wave64_tests`, `valu_tests` | Existen rutas condicionadas por host; RDNA2 física pendiente |
| PSO | `pipeline/make.rs` | Puede descartar pases al superar el presupuesto de espera; un pase único perdido puede afectar toda la escena |
| Trabajadores previos | `effective_workers`, `speculative_limit`, `Queue::pop` | Límite de tres generales y una compilación especulativa en host limitado; demanda tiene prioridad. Flujo continuo de demanda puede retrasar precarga; latencias no medidas |
| Caché previa | `shader_cache.rs`, 256 mutex por hash | Deduplica dentro del proceso, incluyendo validación y reparación; no garantiza deduplicación entre procesos ni comparte errores |
| RAM | `residency.rs`, `texture.rs`, `upload.rs`, `shadow.rs` | Distingue memoria física disponible y commit disponible; falta medir memoria privada del proceso, residente y retenida por trabajador |
| VRAM | `host::video_memory`, `pool::room` | Consulta DXGI local/no local, no presupone 8 GiB libres; el tope de reserva es política, no ahorro medido |
| Vida de recursos | `pool` y retención en listas; fences en `host` | Existe margen temporal REST y control de referencias. Un tiempo transcurrido por sí solo no prueba finalización GPU; revisar con fences y cola retrasada |
| Bindless/texturas | `descriptor`, `draw::heap_textures`, staging y detiling | Falta validar descriptores bajo reutilización, presión y streaming; hay rutas de descarte por recursos ausentes |
| Comandos/sincronización | `queue`, `list`, `host`; esperas de valores entre colas | No se eliminan barreras ni se acortan vidas; pruebas con múltiples colas pendientes |
| Composición/presentación | `compose`, `frame`, `host`, replay | Pacing no demuestra trabajo completo ni FPS efectivo; capturas y timestamps físicos pendientes |
| Vídeo | HLE `media.rs` delega en Media Foundation, resolver local | No desactivar restricciones de URL. Decodificación, A/V y transiciones no verificadas |
| Integración 3D | Pruebas host/replay existentes | No se creó ni ejecutó una escena compleja nueva en este lote; no hay evidencia visual nueva |

Las optimizaciones anteriores no quedan aprobadas por inspección. En particular,
activar automáticamente la admisión cambia cuándo se aplazan texturas; requiere
comprobar que vuelven a solicitarse y que la escena converge bajo presión.

## Corrección: presencia de opciones en la clave de shader

Problema: `VICTORY_STRICT_JOINS` y `VICTORY_MESH_OUTPUT_STRICT` leen presencia,
pero la codificación anterior identificaba ausencia y cadena vacía con los
mismos bytes. Una traducción de un modo podía servirse en otro.

Prueba: la comparación de claves ausente/vacía falla con la codificación
anterior. Se extrae una función pura y se marca el valor explícitamente vacío
con un NUL adicional. Las claves ausentes y no vacías conservan sus bytes;
no cambia la semántica del traductor ni su versión. Dos pruebas verifican la
separación y la compatibilidad. La prueba pura se puede ejecutar sin D3D12.

Limitación: si antes se usaron variables vacías, su antigua clave pudo contaminar
la entrada del modo ausente. Esa caché afectada debe apartarse manualmente fuera
del repositorio antes de comparar modos. No se borran cachés del usuario.

Se añade también una regresión de concurrencia para refresco de un traductor
anterior y reparación de entrada truncada, además del caso frío ya existente.
Requiere compilar y ejecutar el módulo Windows: no se cuenta como superada por
las pruebas portables.

## Corrección: conversión concurrente de la caché general

Hipótesis: varias peticiones frías a `Cache::get_or_convert` observan ausencia
antes de publicar y repiten la conversión y sus temporales.

Una prueba con seis hilos y conversor original de prueba reproduce seis
conversiones donde se espera una. Se añade admisión por 256 mutex indexados por
la clave completa de conversión, con comprobación posterior a adquirir el lock.
El almacenamiento de admisión no crece con el número de entradas. Instancias
separadas de Cache en el proceso comparten admisión. Los aciertos calientes no
adquieren el lock. Las colisiones de stripe sólo serializan, no mezclan datos.
La prueba pasa con una conversión; otra prueba verifica reintento tras fallo.

Límites: sólo dentro del proceso, no entre procesos; los conversores no deben
entrar recursivamente en Cache. El conversor de producción inspeccionado no lo
hace. La caché específica de shaders conserva su lock exterior para cubrir
validación/refresco; ya deduplicaba ese camino, así que este cambio general no
se presenta como una mejora adicional medida de shaders en aplicaciones.
No hay cambios en fences, recursos GPU, formato de entradas ni validación.

## Entorno y reproducción

Linux x86-64 sin `/dev/dri`, sin Windows/MSVC linker/SDK ni RX 6600 accesible.
Se instala Rust en `/workspace/work/{cargo,rustup}`, sin modificar el toolchain
fijado en el repositorio: Rust 1.98.1. Dependencias según Cargo.lock (`--locked`).

```sh
cargo test --locked -p victory-pipeline -p victory-shader
cargo check --locked --target x86_64-pc-windows-msvc -p victory --features diagnostics
```

`cargo test -p victory-gpu` en Linux falla en código heredado: `list.rs` utiliza
`texture::committed` y `host::read_into`, y `resource.rs` usa `host_resource`,
aunque esos componentes sólo existen con cfg(windows). No se insertan respuestas
ficticias a esperas o consultas de recursos para conseguir una compilación verde.

En Windows 11 con herramientas MSVC/SDK y el adaptador físico correcto:

```powershell
cargo test --locked -p victory-gpu --features diagnostics
cargo build --locked --release -p victory
cargo build --locked --profile diag -p victory --features diagnostics
Get-FileHash target/release/victory.exe -Algorithm SHA256
Get-FileHash target/diag/victory.exe -Algorithm SHA256
```

Ejecutables esperados: `target/release/victory.exe` y `target/diag/victory.exe`.
Una comprobación `cargo check` no los genera y no valida enlace ni ejecución.

## Mediciones y criterio para avanzar

| Medida | Antes | Después | Alcance |
| --- | --- | --- | --- |
| Conversiones / 6 peticiones frías simultáneas | 6 | 1 | Fixture CPU propia, no FPS |
| Claves ausente/vacía | Colisionan | Distintas | Prueba de bytes |
| RAM privada / residente / commit | No medida | No medida | Requiere proceso y carga representativa |
| VRAM local/no local y presupuesto | No medida | No medida | Requiere DXGI físico |
| FPS / CPU / GPU / p50,p95,p99 | No medidos | No medidos | No hay escena compleja ejecutada |
| Imágenes / vídeo / transiciones | Sin evidencia nueva | Sin evidencia nueva | Pendiente Windows RX 6600 |

Para las fases B–F quedan necesarias escenas propias con geometría, cámara,
profundidad, iluminación, transparencias, indirectos, streaming y transiciones;
lecturas GPU con valores esperados y comparación de imágenes. Comparar baseline
y experimental a 1920×1080, mismo controlador y condiciones, cachés frías y
calientes, registrando descartes PSO, compilación, transferencias, memoria y
frametimes durante cargas prolongadas. Ningún resultado de este lote demuestra
30 FPS ni corrección gráfica completa. No se cambian resolución ni contenido.

## Resultados de este lote

- `victory-pipeline`: 9 pruebas aprobadas, 0 fallidas, 0 ignoradas.
- `victory-shader`: 210 aprobadas, 0 fallidas, 2 ignoradas. Las ignoradas
  son el benchmark `bench_write` y el caso heredado
  `a_missing_value_stays_missing_through_two_live_joins`, que requiere
  `VICTORY_STRICT_JOINS`. No se oculta esta limitación de control de flujo.
- Codificación de claves: 2 aprobadas ejecutando el módulo puro mediante un
  harness `rustc --test`; con la codificación anterior falla el caso de
  presencia. No se cuentan como pruebas GPU.
- `cargo clippy --locked -p victory-pipeline --all-targets -- -D warnings`:
  aprobado.
- `cargo check` Windows normal: bloqueado en BLAKE3 por ausencia de `ml64.exe`.
- `cargo check --locked --target x86_64-pc-windows-msvc -p victory-gpu
  --tests --features diagnostics,blake3/pure`: aprobado. Incluye comprobación
  de tipos de la regresión de refresco/reparación concurrente; no la ejecuta.
  `pure` se usa sólo en este comando para evitar el ensamblador MSVC ausente;
  no se modifica Cargo.toml, Cargo.lock ni la configuración de distribución.
- No se generan ejecutables Windows de rendimiento o diagnóstico: faltan las
  herramientas nativas MSVC/SDK y la comprobación de tipos no enlaza. No hay
  rutas ni SHA-256 de ejecutables inexistentes. Los SHA-256 de las cinco fuentes
  modificadas están en `docs/rx6600-source-sha256.txt`.

Commits de código: `5a599c7` (claves y regresión de reparación/refresco),
`ab46668` (admisión de conversiones). No se modifica la rama principal.

La comprobación completa del frontend también pasa:
`cargo check --locked --target x86_64-pc-windows-msvc -p victory
--features diagnostics,blake3/pure`. Comprueba el frontend, loader y HLE;
no constituye una compilación enlazada de distribución.
