# pstack. Flujo y buenas prácticas

pstack es el plugin de Cursor de [poteto](https://x.com/poteto). Convierte un chat en un equipo de ingeniería. No maximiza líneas de código. Empuja a escribir menos código, de más calidad, y a probarlo en el artefacto real.

Este documento describe el flujo completo y todas las prácticas que pstack aplica. Cubre poteto-mode, los 23 principios, los 23 playbooks, las skills situacionales, el routing de modelos, la verificación, el shipping y la personalización.

## Cómo se usa

Dos pasos.

1. Instalar el plugin con `/add-plugin pstack` y configurar modelos con `/setup-pstack`.
2. Empezar el trabajo serio con `/poteto-mode`. El resto de skills se invocan solas cuando un paso las necesita.

No hace falta nombrar playbooks ni enumerar skills. Basta un objetivo y una forma de saber que está hecho.

```text
/poteto-mode el export escribe filas duplicadas cuando un retry cae a mitad. repro first, then fix and verify.
```

`/poteto-mode` es un modo persistente. Se queda activo en el chat. Si hay un playbook que encaja o el turno pide rigor, se aplica. Si el turno es casual o el usuario pide salir, no interfiere.

Para cambiar de tema en el mismo chat, dilo.

```text
/poteto-mode new task. averigua por qué la cache sobrevive al logout. no cambies código todavía.
```

## Instalar y configurar modelos

`/setup-pstack` detecta los modelos a los que tienes acceso y escribe `~/.cursor/rules/pstack-models.mdc`, una regla always-applied. Cada skill de pstack la lee. Un rol sin línea conserva el default del skill. Borrar una línea restaura ese default. Volver a correr `/setup-pstack` reescribe el archivo entero.

Los aliases `inherit-parent` y `auto` siempre valen. No son slugs de modelo. Omite el campo `model` del Task y el subagente hereda el modelo del chat padre. Así un usuario en Auto se queda en Auto.

En roles de panel (arena runners, architect runners, interrogate reviewers) el valor es una lista. Se lanza un subagente por entrada. La longitud de la lista fija el fan-out. `arena cross-judge pool` también es una lista, pero Arena elige un valor cuya familia de modelo sea distinta de la del padre cuando puede. `swarm workers` es el modelo por defecto de cada worker, salvo que una carrera asigne un modelo por brazo.

Defaults de fábrica.

| Rol | Default |
|---|---|
| feature, refactoring | `grok-4.6-fast-xhigh` |
| bug-fix, perf-issue, hillclimb | `claude-fable-5-1-thinking-max` |
| judgment and prose | `claude-fable-5-1-thinking-max` |
| hardest tasks | `claude-fable-5-1-thinking-max` |
| how explorer | `grok-4.6-fast-xhigh` |
| how explainer | `claude-fable-5-1-thinking-max` |
| why investigators | `grok-4.6-fast-xhigh` |
| why synthesizer | `claude-fable-5-1-thinking-max` |
| reflect tooling | `gpt-5.6-sol-max` |
| reflect judgment, divergent, synthesizer | `claude-fable-5-1-thinking-max` |
| arena / architect / interrogate panels | Fable, Sol, Grok, Opus |
| swarm workers | `grok-4.6-fast-xhigh` |

Al final, setup busca una forma de probar el comportamiento de la app (`verify-*` o un harness). Si no hay, ofrece una vez `/create-verification-skill`. La regla de modelos aplica a sesiones nuevas.

## El flujo maestro

```text
prompt del usuario
  → poteto-mode lee el índice de principios
  → empareja un playbook
  → abre un todolist con los pasos del playbook copiados literalmente
  → cada paso dispara skills
  → verifica contra el artefacto real
  → responde sin slop, para el consumidor y para el maintainer
  → Opening a PR cierra casi todos los playbooks de código
```

Un paso que se decide no hacer se queda en la lista con `skip: <razón>`. No se borra.

El emparejamiento.

| Señal del pedido | Playbook |
|---|---|
| Pregunta de solo lectura. Cómo funciona X, por qué se construyó Y, ¿estamos seguros? | Investigation |
| Defecto a reproducir, root-causear y arreglar con evidencia de runtime | Bug fix |
| Lentitud medida a trazar y mejorar contra un baseline | Perf issue |
| Mejora científica sostenida de una métrica contra un target | Hillclimb |
| Síntoma live (leak, spin de CPU, glitch) a diagnosticar | Runtime forensics |
| Artefacto de profiling ya capturado | Trace forensics |
| Comportamiento nuevo o cambiado, desde una forma de datos nombrada | Feature |
| Cambio que preserva comportamiento (rename, extract, inline, move) | Refactoring |
| Boceto desechable para decidir diseño o comportamiento | Prototype |
| Equivalencia pixel-exacta entre dos implementaciones | Visual parity |
| Escribir o editar un SKILL.md | Authoring a skill |
| Probar a ciegas cómo un cambio de skill o prompt altera al agente | Eval |
| Llevar un PR o un stack a merge-ready | Babysit |
| Verificar un stack verde y aterrizar la racha verificada de abajo arriba | Shipping |
| Tarea larga hasta un predicado, sin parar | Autonomous run |
| Programa de varios días, muchos PRs, flota de subagentes | Orchestrate |
| Cola de PRs independientes hasta merged, un owner por PR | Autopilot-full |
| Cola construida y verificada como stack lineal que el operador aterriza | Autopilot-stack |
| Retomar trabajo a medias de otro agente | Session pickup |
| Suspender limpio para retomar después | Pause safely |
| Trabajo que abarca fases o PRs apilados | Multi-phase plan |
| Liberar disco. Worktrees y simuladores iOS | Worktree cleanup |
| Abrir el PR. Se invoca al final de los demás | Opening a PR |

Trabajo grande o que el usuario dejará para revisar después va a `figure-it-out` aunque Feature encaje. `figure-it-out` diseña una corrida a medida. Un programa de escala de proyecto va a Orchestrate. `figure-it-out` diseña una corrida. Orchestrate corre el programa.

## Non-negotiables

Estas reglas se aplican en cada tarea que dispara poteto-mode.

1. Nombrar en la respuesta cada principio que cambió una decisión. Citar solo principios cuyo SKILL.md hoja se leyó en esta sesión.
2. Cambio no trivial, decisión de arquitectura o "¿estamos seguros?" → skill **how**.
3. Antes de AskQuestion sobre un fork de enfoque, clasificar. Si la respuesta es un hecho que se puede observar (comportamiento, timing, layout, output, perf, si un eval separa), no es pregunta para el humano. Bocetarlo con Prototype y dejar que el resultado decida. Si la tarea es Investigation de solo lectura, responder desde la evidencia. Reservar la pregunta para una preferencia de producto que ningún experimento zanja.
4. Cualquier código → nombrar primero la forma de los datos y elegir su estructura según **model-the-domain**.
5. Código que cruza un límite de función → **architect**, exploración de diseño en paralelo antes de implementar.
6. Fan-out paralelo → **swarm** para matrices de cobertura, carreras, gauntlets y particiones. **arena** para bakeoffs de diseño o código, con selección de base y grafting.
7. Diseño contestado → **interrogate** (adversarial multi-modelo) antes de shippear.
8. Multi-paso no trivial → escribir el throughput checkpoint (Feature, paso 3).
9. Cualquier superficie de prosa → **unslop**. La respuesta del agente es una superficie de prosa.
10. Docs, RFCs, readmes, PRs, commits → **technical-writing**.
11. Antes de commit → `deslop` del plugin `cursor-team-kit`.
12. Antes de review → **no-comments**.
13. UI / IDE / CLI → el control skill que corresponda (`control-cli` o `control-ui` en cursor-team-kit). En bugs, reproducir primero en esa superficie. Entregar el repro al usuario solo en la excepción estrecha del paso 1 de Bug fix.
14. Pedido de estado de PR → playbook **Babysit**, no el babysit built-in de Cursor. Abrir un PR no dispara babysit.
15. Pedido de aterrizar o shippear un stack verde → **Shipping**. Verde no es seguro.
16. Comentario de Bugbot o security review → postura escéptica. Triage `fix` / `dismiss` / `ask` según `bugbot-triage.md`.
17. Skill roto a mitad de tarea → arreglarlo en su propio PR. No bloquear. No rodearlo en silencio.
18. Trabajo largo, autónomo, multi-fase, o que el usuario dejará para revisar después → trail de decisiones con **show-me-your-work**.

## Autonomía

Trabajo reversible y acciones externas (chat de equipo, tickets, evals) se hacen sin preguntar.

Siempre pausar en escrituras irreversibles. Force-push a ramas compartidas, deploys, borrado de datos, mensajes a clientes.

"Don't stop", "going to bed", "run until done", "be fully autonomous" son overrides de sesión. Seguir.

No es una respuesta aceptable. Si te preguntan si hacer algo, si te invitan a ampliar scope o si te muestran un enfoque, responde con juicio real. Declinar, empujar o decir "esto no se gana su sitio" cuando sea verdad. El acuerdo no es el default.

## Subagentes

Usa `subagent_type: "poteto-agent"` para cualquier subagente que lances dentro de un paso de playbook. `/poteto-mode` y `poteto-agent` pasan por el mismo wrapper. `poteto-agent` lee poteto-mode entero, incluido el índice de principios, antes de trabajar. Sustituirlo por `generalPurpose` se salta esa lectura y deriva.

Las skills de workflow (`how`, `why`, `interrogate`, `reflect`, `swarm`) fijan su propio `subagent_type` para review con modelos diversos. Respeta lo que prescribe el skill. No lo sobrescribas a `poteto-agent`.

Defaults de cada Task.

- `run_in_background: true`
- Modo agent. Readonly quita MCP.
- Punteros a archivos, no contexto inline.
- Modelo explícito por rol, configurable con `/setup-pstack`.
- Código mecánico trivial → modelo rápido (Grok).
- Cambios duros, concurrencia, algoritmos sutiles → modelo de juicio más fuerte (Fable), tanto si el intent es vago como si el brief es una secuencia precisa.

El padre es dueño del trabajo de cada subagente. Revisa el diff y escribe tu propio resumen. No reenvíes lo que dijo. Un resume encadenado por interrupt pierde directivas. Lanza un subagente fresco con scope consolidado. Una segunda opinión es el mismo prompt contra otro modelo. El acuerdo es señal alta.

## Cómo se responde

Escribe limpio al redactar. Un pase de limpieza después no quita estos patrones.

- Oraciones cortas y declarativas. Un pensamiento por oración.
- Sin raya larga en ningún sitio.
- Dos puntos solo antes de una lista o un ejemplo. No como conector a mitad de frase.
- Terse no es excusa para recortar contenido. Cada sección que el playbook nombra se queda.
- Enmarcar impacto para el consumidor y para el maintainer. Quién usa el trabajo y qué cambia para esa persona, antes de cualquier detalle de implementación. Luego qué hereda el siguiente ingeniero. Si no puedes decir qué notaría cada uno, el trabajo o la explicación está mal.
- Nunca fabricar un link, una cita o una referencia a un transcript. Enlazar solo artefactos producidos o leídos en esta sesión.
- Cada afirmación lleva su evidencia o su etiqueta en la misma oración. Measured, inferred o guess. Una predicción o una causa no vista es guess. Nunca entregar al humano un check que tú podrías haber corrido.

Cada playbook termina con una respuesta escrita así. El link del PR va como `https://github.com/<owner>/<repo>/pull/<number>`.

## Comentarios en código

La misma regla que la respuesta. Un comentario se queda solo para un *por qué* no obvio que el código no puede mostrar. Un script de verify o un test no lleva comentarios que narran fases (`// Phase 1: add cards`). La aserción o el string del log documentan el paso, como `assert(ok, 'persisted across restart')`. Esto aplica a cada archivo que produzcas, incluido el diff del delegate.

## Los 23 principios

`/poteto-mode` lee el índice al empezar una tarea multi-paso. Aplica los que el trabajo dispara. En la respuesta nombra cada uno y la decisión concreta que cambió. Una cita sin decisión es name-drop, no aplicación.

No se invocan. Se usan sus nombres para redirigir.

```text
apply prove it works. corre el import real y enséñame los records escritos.
```

Lee el SKILL.md hoja de cada principio que apliques.

### Núcleo

**Laziness Protocol.** Al refactorizar, al medir el tamaño del diff, o cuando tienta añadir abstracciones, capas o threading de señales. Sesgo a borrar y al cambio más pequeño que resuelve el problema. Jerarquía de llamadas plana. Si responder una pregunta exige más de 3 archivos o capas, aplanar. Consolidar decisiones detrás de una sola fuente de verdad. Sudar las fugas chicas (pass-throughs, leaks de representación, elecciones duplicadas). La prueba. Si un humano encontraría el código agotador de mantener, es una mala solución.

**Foundational Thinking.** Antes de escribir lógica. Tipos y estructuras de datos primero. Trazar cada patrón de acceso. DRY la estructura, no cada línea. Tres statements parecidos ganan a una abstracción prematura. Antes de compartir estado entre actores, preguntar qué pasa si otro lo modifica a la vez. Si la respuesta no es "nada", aislar. Scaffold primero (CI, lint, tests, tipos compartidos). Resta restar código muerto antes de andamiar.

**Redesign from First Principles.** Al integrar un requisito nuevo en un diseño existente. Rediseñar como si hubiera sido una asunción fundacional desde el día uno. Leer todos los archivos afectados. Preguntar qué construiríamos desde cero con este requisito. Propagar el cambio a tipos, docs, ejemplos y rationale. Pensar el rediseño entero y entregarlo por incrementos.

**Attack the Premise.** Cuando dos o más fixes que comparten una premisa fallaron el mismo gate. Escribir la premisa en una oración. Hacer un censo rerunnable de qué actores sostienen el desequilibrio. Si los mismos actores cargan el sesgo en cada run, encontrar qué les asigna ese rol y quitar la asimetría. No compensarla con return paths, pools compartidos o rebalances. Si el censo es parejo, la premisa no es la causa.

**Subtract Before You Add.** Al secuenciar una adición, un refactor o un rewrite. Quitar peso muerto, validators redundantes y stubs primero. Cortar antes de pulir. Diseñar para el uso observado, no para edge cases especulativos. Dejar el diseño un poco más simple y capaz, con la misma o menor superficie.

**Minimize Reader Load.** Al revisar o dar forma a código difícil de seguir. Dos ejes. Capas entre la pregunta y la respuesta. Estado oculto o mutable que el lector debe guardar en la cabeza. Colapsar wrappers de un solo caller y adapters sin segunda implementación. Cada capa debe cambiar la abstracción. Encoger el scope mutable. Funciones puras sobre mutaciones, locals sobre fields, fields sobre estado de módulo. Nombrar el invariante en el límite, no en cada consumidor. La prueba. Un lector nuevo responde "¿de dónde viene X?" y "¿qué puede cambiar X?" en menos de 30 segundos.

**Outcome-Oriented Execution.** Rewrites y migraciones planificadas con fronteras de fase explícitas. Converger a la arquitectura destino. No preservar estados intermedios suaves con código de compatibilidad desechable. La rotura intermedia es aceptable si está planificada, acotada y es reversible. Verificación estática y de runtime completa al terminar.

**Experience First.** Tradeoffs de producto, UX o scope. Elegir el deleite del usuario sobre la conveniencia de implementación. Enviar menos features pulidas, no más features toscas. Prototipar antes de comprometer. El usuario es quien consume el trabajo. En una UI, el usuario final. En una librería, el colega que la importa. El siguiente maintainer también es usuario.

**Exhaust the Design Space.** Interacción o decisión arquitectónica sin precedente. Construir 2 o 3 prototipos que compiten de verdad y compararlos lado a lado. Un segundo sabor de la primera forma no cuenta. No aplica a bugs, refactors con destino claro, ni a cambios donde las constraints dejan un solo camino.

**Build the Lever.** Cualquier trabajo no trivial. Construir la herramienta que lo hace o lo prueba. Codemod, script, generador, o un skill que los subagentes siguen. El reviewer rerunea esa herramienta. Hacer la primera unidad a mano para aprender la receta. Luego construir la herramienta y diferenciarla contra esa unidad. Un lever determinista gana al fan-out. Si citas este principio y no hay un archivo de herramienta en el diff, no lo aplicaste. El listón es trivialidad, no repetición. Un one-off también gana un lever si eso hace el trabajo comprobable.

### Arquitectura

**Model the Domain.** Lógica con estado, mucho branching, o una asunción de forma repetida entre archivos. Codificar el dominio en una estructura. Máquina de estados, modelo tipado, tabla o registry, reducer, unión discriminada, cola, índice, grafo. No forzar abstracción. El código aburrido se queda si la forma ya es clara, local y estable. La señal de que lo saltaste es un if/else que crece una rama más, o un segundo boolean que debe quedarse en sync con el primero.

**Boundary Discipline.** Validación, errores o adapters de framework. Guards en los límites del sistema (CLI, config, red, APIs externas). Confiar en los tipos internos. Lógica de negocio en funciones puras. El shell es delgado y mecánico. No reexportar tipos de transporte, storage o framework por la superficie pública. No revalidar nil profundo si el límite ya validó.

**Type System Discipline.** Al diseñar tipos o una firma en cualquier lenguaje tipado. Hacer estados ilegales irrepresentables. Brandear primitivos semánticos. Parsear datos externos en el límite. No mentirle al compilador (casts, coerciones, asserts). Exhaustar variantes. Derivar tipos de schemas autoritativos. Fortalecer un tipo solo donde aparece partiality. `{ completed: boolean; completedAt?: Date }` admite combinaciones sin sentido. Un `UserId` y un `OrderId` no son intercambiables.

**Make Operations Idempotent.** Comandos, pasos de lifecycle o loops que corren entre crashes y retries. Converger al mismo estado final. Startup convergente. Cleanup por equivalencia de contenido, no por orden de creación. Locks que se auto-sanan por PID. Scheduling que respawnea trabajo fallido. Tres preguntas. ¿Qué pasa si corre dos veces? ¿Qué pasa si el run anterior crasheó en cada punto posible? ¿La reejecución converge?

**Migrate Callers Then Delete Legacy APIs.** Al introducir una API interna nueva con callers viejos. Inventariar, migrar y borrar en la misma ola. Adapters temporales son excepción acotada en el tiempo, no arquitectura default. Actualizar tests al contrato nuevo. Borrar tests que solo protegían detalles de la implementación anterior. Aplica cuando no hay usuarios externos que dependan de compatibilidad.

**Separate Before Serializing Shared State.** Actores concurrentes que podrían escribir el mismo archivo, rama, key u objeto. Eliminar el sharing primero. Dar a cada actor su propio archivo, key, rama o directorio. Merge solo en el límite de lectura o reporte. Dos workers escribiendo campos distintos en un `state.json` sigue siendo mutación compartida. Serializar estructuralmente (lockfile, fase secuencial, single-writer, CAS) solo cuando un writer compartido es un invariante real. Instrucciones y convenciones no son control de concurrencia. Un lock es un smell a investigar, no la primera respuesta.

### Verificación

**Prove It Works.** Después de una tarea, antes de declarar done. Verificar contra el artefacto real. Correr la feature, leer el valor real, inspeccionar el diff. No un proxy, un self-report o "compila". Desconfiar del método de observación antes de desconfiar del sistema. En trabajo delegado, inspeccionar el artifact (diff, contenidos, runtime), no el resumen del delegate. El proof más fuerte es un script determinista que el reviewer puede rerunear.

**Fix Root Causes.** Al debuggear. Reproducir primero. Preguntar por qué hasta el cause. No añadir nil-checks que silencian crashes. Si un workaround necesita un comentario de un párrafo, el código está mal. Grep el patrón, no solo la instancia. Cuando estás atascado, instrumentar. No adivinar. Bugs de restart. Sospechar estado persistente stale (config, caches, locks) antes que el código.

**Sequence Work into Verifiable Units.** Trabajo multi-paso y cómo se apilan commits y PRs. Unidades chicas que terminan en un check. Verificar cada una antes de la siguiente. Ordenar la entrega para que la secuencia se pruebe sola ante un reviewer. Forma canónica. Test que falla primero, fix encima. Otras formas. Resta antes de reshape. Baseline antes de treatment. Scaffold antes de feature. Nunca verificación en batch al final.

**Test Behavior, Not Implementation.** Al escribir, cambiar o conservar un test. Llamar el código como lo llaman sus usuarios y asertar el resultado que observan contra un valor literal esperado. Si el test seguiría pasando cuando cada función importada devolviera `undefined`, reescribir la aserción o borrar el test. Cinco formas que fallan esa prueba. Aserción débil o ausente. Solo mock o ausencia. Valor esperado autorreferencial. Pin de una constante. Fixture que aserta el fixture. Conservar tests de relación entre filas de tabla y checks compile-time en `*.test-d.ts`.

### Delegación

**Guard the Context Window.** Cuando el contexto se llena. Rutas de payload grande a subagentes. En el hilo principal, resúmenes, no dumps. No leer lo que no vas a usar. Templates de uso frecuente van inline en el skill, no en archivos aparte. Acotar fases y poner presupuestos de turnos y archivos.

**Never Block on the Human.** Tentación de preguntar "¿hago X?" en trabajo reversible. Proceder, presentar el resultado, dejar que el humano corrija después. Reservar confirmación para acciones irreversibles. La dirección de producto viene del humano. La ejecución no se bloquea.

### Meta

**Encode Lessons in Structure.** Cuando te pillas escribiendo la misma instrucción por segunda vez. Codificarla como lint, flag de metadata, check de runtime o script. Borrar la instrucción. Elegir el mecanismo más fuerte que la situación permita. Estado irrepresentable que no compila gana a lint que falla CI, que gana a helper canónico, que gana a check de runtime. Una corrección one-off va a una nota. Una corrección recurrente va a un skill o un lint. Un issue sistémico va a un principio. "I'll keep that in mind" no persiste.

## Los 23 playbooks

Abre el archivo del playbook y copia sus pasos literalmente al todolist. Lo que sigue es el flujo de cada uno.

### Investigation

Solo lectura. Explicación citada o recomendación. No cambia código.

1. Enrutar por **how**. Si la pregunta es de motivación, también **why**.
2. Throughput checkpoint en una línea. `throughput checkpoint: n/a, read-only investigation`.
3. Salida con forma how (Overview, Key Concepts, How It Works, Where Things Live, Gotchas), o recomendación con tabla de tradeoffs.
4. Aplicar **unslop**.

Sin PR, sin babysit, sin architect, salvo que la investigación preceda un cambio. En ese caso, devolver al usuario y re-enrutar a Bug fix o Feature. En "¿estamos seguros?", juicio real. Empujar si la premisa es incorrecta.

### Bug fix

Cada línea enviada traza a evidencia de runtime. Un "belt-and-suspenders que podría ayudar" es hipótesis, no fix. Si la evidencia refuta la hipótesis, revertir lo motivado.

1. Reproducir en la superficie correspondiente con el control skill. El agente conduce el runtime instrumentado. Preguntar al usuario solo después de agotar esa superficie, y solo con una razón concreta de que no alcanza el target. Si no reproduce, forzar. Sintetizar el trigger, ajustar condiciones o instrumentar hasta que dispare.
2. Búsqueda binaria de la causa. Hipótesis candidatas. Descartar hasta que una sobreviva. Sembrar con **how** sobre el subsistema y **why** para historial de regresiones. Cada paso toma el split que más reduce el espacio, obtiene evidencia de runtime y elimina. Si el estado del programa no está claro, añadir logging y leerlo mientras corre. Confirmar el mecanismo sobreviviente con evidencia de runtime antes del fan-out de architect o interrogate.
3. Planear el fix. Si cruza un límite de función, **architect** primero. Delegar implementación al subagente bug-fix (default Fable) con scope concreto. Revisar el diff.
4. Verificar en la misma superficie. El repro original ahora pasa. "Inconclusive" o superficie incorrecta no es pass. Un unit test muestra comportamiento de rama, no ausencia del bug.
5. Staging. Repro fallando antes del fix en el historial git. **tdd** si hay un test local barato. Skip si el test sería caro, de integración pesada o unclear. Esto es **sequence-verifiable-units**.
6. Opening a PR.

Respuesta. Qué estaba roto, root cause, fix, cómo se verificó. Pegar el output del repro failing-then-passing verbatim.

### Perf issue

Cada fix se ata a una medición. No leer source en lugar de medir.

1. Capturar baseline trace con el control skill.
2. **how** para fundamentar hipótesis. Ocho familias, no un checklist. Solo intentar las que el trace señala. Elimination, divide and conquer, caching (nombrar qué invalida), indirection, batching, redundancy, lazy evaluation, scheduling.
3. Planear el fix desde el trace. Architect si cruza un límite. Delegar al subagente perf-issue (default Fable). Trace post-fix. Verificar cada intento antes del siguiente.
4. Parsear y comparar artefactos. Inconclusive no es pass.
5. Citar la medición en el PR.
6. Opening a PR.

Respuesta. Número baseline, número post-fix, delta, ruta del artefacto. Mejora sostenida de una métrica no es esto. Eso es Hillclimb.

### Hillclimb

Un cambio, una medición, keep o revert. Nunca apilar cambios no testeados. Nunca reclamar un win por inspección de código.

1. Ground del workload y la arquitectura con **how**. Una métrica, dirección de "mejor", predicado de stop verificable (target más un piso de intentos). Si ningún caso reproduce la queja, arreglar el repro, no hillclimbar.
2. Construir el harness de medición, probar sensibilidad, congelarlo (**build-the-lever**). Un comando repetible emite la métrica, muestreada lo suficiente. Registrar baseline y green run del regression gate antes de cualquier cambio.
3. Abrir `decision.tsv` con **show-me-your-work**. Una fila por intento. Fuera del tree.
4. Cada hipótesis nombra un mecanismo específico.
5. Loop. Una hipótesis por iteración. Delegar al subagente hillclimb. Hipótesis independientes en worktrees separados. Medir before/after. Aceptar solo si la métrica supera el ruido y el gate sigue green. Si no, revert completo. Un commit por win aceptado, `git add <files>`, nunca `-A`.
6. Empujar más allá del primer plateau. En stall, pivotar categoría, combinar near-misses, intentar algo más radical. Correctness y simplicidad ganan al número.
7. Parar cuando el predicado se cumple. No relajar el predicado. Si estás stuck, surface, no spin.
8. Opening a PR con los commits aceptados en orden de landing.

Respuesta. Métrica y target. Baseline → final con porcentaje. Iteraciones kept vs reverted. Cada fix aceptado en una línea. Path de `decision.tsv`. La mejor idea si se empujara más.

### Runtime forensics

Diagnóstico citado. No es un fix.

1. Capturar señal live (CPU profile, heap snapshot, CDP trace). Artefacto real.
2. Reducir al smoking gun. Parsear artefactos grandes en un subagente. Finding reducido en el hilo principal.
3. Probar el mecanismo. Instrumentación vía CDP eval o hotfix live, sin reload.
4. Mapear a source. File, symbol, línea.
5. Throughput checkpoint. `n/a, read-only forensics`.

Luego handoff a Bug fix o Perf.

### Trace forensics

La captura ya existe. Es un dataset fijo. Leerla, no re-ejecutarla.

1. Identificar formato y cargar con la herramienta correcta.
2. Transformar a forma consultable (sqlite, una fila por sample, frame o node) antes de leer.
3. Narrow to cause. Frames con más tiempo, retainer chain, thread stuck y wait reason.
4. Atribuir a source. Un frame sin source mapping no es diagnóstico todavía.
5. Confirmar contra captura emparejada si existe. Sin ella, marcar el finding como la hipótesis más fuerte que el artefacto soporta, no como causa confirmada.
6. Handoff a Bug fix o Perf.

### Feature

El padre es dueño del diseño. Delega implementación.

1. **how** sobre el subsistema.
2. **architect** para exploración paralela. Skip queda como `architect skipped: <razón>`. No doblar la decisión de diseño en silencio dentro de la implementación.
3. Throughput checkpoint como cuatro todos. Una dimensión que no aplica se queda con `n/a: <razón>`.
   - Blocking first steps.
   - Independent workstreams.
   - Shared mutable state. Default, split. Serializar solo por invariantes reales.
   - Smallest safe decomposition. Si un worker es mejor, nombrar por qué.
4. Delegar al subagente feature (default Grok) con scope concreto. Paths, data shape y estructura organizadora ya elegidos, criterios de éxito. Revisar el diff. Si la implementación admite varias formas válidas, delegar vía **arena**. Esto es mandatory. Laziness Protocol no lo anula. El subagente puede spawnar aunque él mismo sea subagente. "La app es chica" y "un subagente no puede spawnar" son incorrectos. Comments según la regla de Comments. Edits quirúrgicos. Portar mejoras de primitivos compartidos a todos los consumers y verificar cada uno. Commit liberally.
5. Verificar en la superficie correspondiente. Inconclusive no es pass.
6. Rebase en commits chicos ordenados. **sequence-verifiable-units**.
7. Si el diseño está contestado, **interrogate** antes de shippear.
8. Opening a PR.

Respuesta. Qué se construyó, qué se eligió y por qué, throughput checkpoint, open decisions. Tablas para alternativas.

### Refactoring

La estructura cambia. El comportamiento no. Si el cleanup revela una feature faltante o un bug real, split out. Shippear el cambio estructural primero contra el contrato pinned.

1. Pin del contrato de comportamiento. **how**. Characterization test, snapshot o equivalence harness antes de mover estructura. Type check y lint no son pin.
2. Nombrar la estructura que falta según **model-the-domain**. El reshape debe eliminar branches o estados inválidos, no añadir indirección.
3. Nombrar el target shape. Si cruza un límite de función, **architect**.
4. Subtract before you add. El cambio más pequeño que alcanza el target. Cleanup especulativo se revierte.
5. Mover en pasos chicos que preservan comportamiento. Cada uno mantiene el pin green. Migrar todos los callers y borrar la API vieja en la misma ola. No shims. No paths old-and-new. Spot-check cada rename contra archivos reales. Delegar edits mecánicos al subagente refactoring (default Grok).
6. Probar que el comportamiento no cambió en el artefacto real. En reshapes grandes, equivalence check.
7. Confirmar que vale la pena. La medida de éxito es menor reader load. Si el diff no la baja, revertir.
8. Rebase. Resta → reshape → cleanup. Opening a PR.

Respuesta. Estructura cambiada, pin, proof de equivalencia, delta de reader load, qué shipped y qué se revirtió. Sin comportamiento nuevo.

### Prototype

El único playbook donde Laziness Protocol y la barra de verificación se invierten. Velocidad gana a polish. La calidad de código no importa. El rigor está en elegir el diseño barato.

1. Scope de la decisión. Sin decisión, no es prototype. Enrutar a Feature.
2. Recoger referencias cuando el espacio está abierto. Skip si la dirección ya está fija.
3. Construir throwaway en un scratch dir aislado. Visual. HTML/CSS/JS vanilla, CDN, hot reload. Behavioral. El script más chico que ejercite la pregunta. Sin framework de producción, tests ni abstracciones.
4. Alternativas detrás de un switcher, cada variant labeled.
5. Verificar. Visual, screenshot de cada variant. Behavioral, observación. Observación es el test.
6. Presentar alternativas, tradeoffs y recomendación. Handoff a Feature o architect.

Decir explícitamente que el prototype es throwaway.

### Visual parity

La baseline es la spec. No se toca. Equivalencia por image diff, no a ojo.

1. Establecer baseline primero. Harness que screenshot el componente en sus states. Sin baseline no hay claim de parity.
2. Anti-shortcut. No modificar el harness. No tamper la baseline. No reestructurar el componente para pasar el diff. Si la baseline se ve mal, parar y preguntar.
3. Migrar un componente a la vez. Worktrees paralelos. Primitivos compartidos primero, fase bloqueante.
4. Verificar cada componente. Diff nonzero es fail. `/loop` por componente hasta diff zero.
5. Opening a PR por componente o batch seguro.

### Authoring a skill

1. Usar el built-in **create-skill** de Cursor.
2. Validar. Frontmatter con `name` y `description`. Archivos referenciados existen. Links entre skills resuelven.
3. Test cases si es estructural. Skip si es subjetivo.
4. Opening a PR.

Cuando dudes, borra. Solo prosa que cambia una decisión. Decirle que haga la cosa. Saltar la razón salvo que la regla sea confusa sin ella. Apuntar a fuentes estructurales. Delegar a otros skills por path. No re-enunciar.

### Eval

Diseño del experimento, blinding, run, síntesis.

Non-negotiables de blinding. El candidato no ve las palabras `eval`, `test`, `judge`, `experiment`, `rubric`, `score`, `compare`, `benchmark`, `candidate` ni `arena` en directorio, archivo o prompt. El prompt es un pedido orgánico de usuario. No pedir al candidato que liste skills o principios aplicados. Sanitizar nombres. No decirle que existen otros candidatos. El judge puede saber que juzga, pero ve outputs por label sanitizado, nunca por nombre de modelo.

1. Frame. Variant bajo test, comportamiento que es success, rúbrica de 3 a 6 criterios solo para el judge.
2. Environments sanitizados por candidato.
3. Un prompt orgánico.
4. N candidatos en paralelo, Phase B de arena.
5. Un judge ciego, Phase C de arena.
6. Verificar la cadena desde transcripts, no desde self-report. Leer qué archivos abrió cada candidato. No glob across `~/.cursor/projects/*/`.
7. Leer cada output end to end. Comparar con el veredicto del judge. Sintetizar.

Respuesta. Variant, rúbrica, notas por candidato, veredicto del judge, síntesis, si promover.

### Babysit

Dueño de la frontera de merge. Declara un modo, limpia un PR a la vez, para donde empieza la llamada del humano. Reemplaza el babysit built-in de Cursor. Pedido de land o ship → Shipping.

1. Declarar modo y resolver forge antes de cualquier poll.
   - `drive`. Loop hasta merge-ready.
   - `background`. Triage sin bloquear, el plan sigue ejecutando.
   - `threads-only`. Responder review comments, no tocar nada más.
   - `check`. Un pase de status y report.
   - Sin declarar, default `drive`. PRs chicos o solo docs van a `check`. GitHub CLI (`gh`) es default. Si `origin` resuelve el repo, usar Origin. Nunca exigir Graphite (`gt`).
2. Trabajar solo la frontera de merge. El PR más bajo sin mergear es el único que importa. Threads upstack se leen y se bachean. Nunca se arreglan a costa de reiniciar checks de la frontera.
3. Un babysitter por stack.
4. Nunca mutar la topología del stack. No retarget de base, no rebase, no submit de stack, no force-push desde babysit. Conflicto se reporta, no se resuelve. La única creación sanctionada es un PR nuevo encima del stack restante cuando el PR dueño del fix ya mergeó.
5. Orden. Conflicts → review threads → CI. Bachear cada fix conocido en una sola push wave.
6. Confiar en el veredicto del forge activo, no en una lista de checks verdes. `drive` y `background` bajo `/loop`. El watcher despierta. Nunca un segundo sleep loop. Nunca mergear ni armar merge-when-ready desde babysit.
7. Clasificar CI antes de retrigger. Flake o infra, un build fresco, nunca retry de job. Un solo retry. Segunda falla idéntica no es flake. Falla en código que el diff no toca, base stale. Solo la falla en el código propio del diff recibe commit.
8. Bugbot con postura escéptica. Ver `bugbot-triage` más abajo. Nunca churn de código para callar al bot.
9. Parar en la línea del humano. Approval del owner es espera, no blocker para arreglar. After merge-ready, un sweep de patrones de dismiss útiles para el equipo.

Respuesta. Modo, frontera y estado del forge, tabla de 4 columnas del watcher en GitHub, fixed vs dismissed con razones, pendiente, qué necesita el humano.

### Shipping

Dueño de lo que aterriza. Verde no es seguro. Nada se arma antes de un veredicto independiente por PR. Solo aterriza la racha verificada continua desde la raíz.

1. Resolver forge. Un subagente por PR, no bacheados. Cada uno ejercita la superficie real. Devuelve `PASS`, `PASS+NOTES` o `FAIL`. Seguro es un veredicto de un agente que no escribió el código. CI green no es veredicto. Review de bot aprobando no es veredicto.
2. Aterrizar solo la racha verificada continua desde abajo. Parar en el primer PR sin veredicto passing. Un PR verificado encima de uno no verificado no es landable.
3. Re-chequear que el veredicto todavía describe el patch. SHA de head, SHA de base, `git patch-id` estable. Si el patch cambió, re-verificar. Si no cambió, conservar el veredicto de código y re-correr mergeability y CI en el head actual.
4. Preparar solo el PR de abajo. Rebase sobre el tip exacto de trunk. Push. Retarget solo ese PR a trunk. No retarget, armar ni mergear descendientes todavía.
5. Aterrizar de a uno. Squash merge. Si el usuario pidió merge-when-ready y los requirements siguen corriendo, armar solo ese PR.
6. No leer `autoMergeRequest` de GitHub como readiness de stack.
7. Recomputar después de cada merge.
8. Watch de la frontera actual hasta que mergea o falla. No mutar la cola alrededor.
9. Parar en el techo. Reportar qué aterrizó, el siguiente PR no verificado, qué haría falta para verificarlo.

### Autonomous run

Dueño de la condición de salida.

1. Enunciar el predicado chequeable antes de la primera iteración.
2. Elegir mecanismo de wake con `/loop`. Evento más heartbeat largo de fallback, o intervalo fijo si no hay evento.
3. Cada iteración. El cambio más pequeño que la evidencia justifica. Verificar contra el predicado. Commit si avanzó. Descartar lo que no ayudó.
4. Descubrimientos mid-run son tuyos. Skills rotos, bugs relacionados, verifiers flaky, ruido de review. Fixes out-of-band en su propio PR. No aparcar trabajo reversible para el humano. Surface solo acciones irreversibles, llamadas de producto que ningún experimento zanja, o un dead end real.
5. Checkpoint cada iteración en **show-me-your-work**.
6. Parar cuando el predicado se cumple. Plateau no es stop. Nunca relajar el predicado para declarar victoria.

### Orchestrate

Dueño del programa, nunca del código. Un chat coordinador standing. Multi-día, muchos PRs apilados, decenas o cientos de subagentes. El humano mira unas dos veces al día.

Tres reglas. Completions son eventos de cola, no interrupts. Cada spawn y resume lleva standing orders verbatim. El brief es el producto. Un brief vago falla en silencio.

Roles.

- Coordinador (este chat). Framea, briefea, drena inbox, reporta, decide. Nunca escribe código. Landing mecánico de una unidad verificada en un git local barato sí puede ser bookkeeping.
- Sub-coordinador. Uno por track cuando el programa excede un drain. Cap in-flight ~10. Profundidad 3.
- Worker o verifier. Siempre `environment: "cloud"` salvo que necesite esta máquina. Un writer por worktree o rama. El verifier usa una familia de modelo distinta.

Store en `orchestrate/<project-slug>/`. `preferences.md`, `overview.md`, `units.tsv`, `frontier.json`, `ledger.tsv`, `inbox/`, `gates.md`, `decisions.tsv`, `status.md`. CLI `bun scripts/orch/orch.ts`.

Brief. GOAL, SCOPE, CONTEXT, ACCEPTANCE, VERIFY, TIMEBOX, FORBIDDEN, REPORT, STANDING. Campos faltantes, refuse-to-spawn.

Pasos. Frame (predicado contable, landing ~70% del presupuesto). Install runtime. Pilot de una unidad por todo el path. Scale. Drain. Land continuo. Close.

Ledger de verificación. Filas keyed por PR + head SHA. `live-ui-verified | unit-test-verified | type-check-only | verifier-blocked | verifier-failed`. CI green es input, no veredicto. Un SHA nuevo anula la fila.

Llega al humano. Acciones irreversibles, llamadas de producto, standing order que contradice la realidad, dead end de programa. No llega. Nudges de frontera, restacks, retries, flake de CI, triage de threads, "¿sigo?".

Respuesta con números de las tablas, no de la narrativa.

### Autopilot-full

Dueño de los veredictos, nunca de los PRs. Un owner corre cada PR desde build hasta merge. Nada mergea sin un swarm verdict limpio del root.

1. Marcar items del operador. State-then-wait. Ejecución solo con go explícito. Armar `/goal`.
2. Un owner cloud por PR. Build, primer push, PR ready never draft, self-proof en el artefacto real, triage escéptico de Bugbot, deslop, no-comments, rebase sobre trunk, babysit a green, merge (gateado por el paso 4). En ~15 min, `decisions.tsv` y PR abierto.
3. Owners en paralelo real. Nunca stack salvo overlap genuino.
4. Swarm-verify cada head merge-ready. Lanes. Re-correr gates. Probar comportamiento load-bearing live. Auditar receipts y diff, desconfiar del body del PR. Regression lane contra trunk. Live lane es el piso. Findings vuelven al owner. Head nuevo, swarm fresco.
5. Con veredicto limpio el owner mergea y toma el siguiente item. Regla de patch-id si trunk se movió. Items del operador paran en merge-ready.
6. Capa root. Audit tick ~cada 30 min. Releer el playbook desde trunk. Solo side effects cuentan como progreso. Lane sin side effect pasado el runtime esperado, stand down y reemplazo.
7. Hold o stand-down del operador, zero-writes inmediato.

### Autopilot-stack

Hermano de Autopilot-full. El stack se construye y verifica. El operador lo aterriza.

El loop de owners es el mismo, sin merge. El root es el único writer de topología. Append con rebase al tip exacto del parent y `--force-with-lease` tras `ls-remote`. Solo el PR root apunta a trunk. Nunca registrar la cadena por `gt`. Drift se absorbe en el root y se re-verifica lo que se movió. Entregable. Una cadena lineal de PRs verificados, reviewable de abajo arriba.

Elegir. Autopilot-full si los PRs son independientes y hay autoridad de landing. Autopilot-stack si el operador quiere review antes de aterrizar, o el trabajo está acoplado.

### Session pickup

Dueño del punto de resume. Leer el trail previo, no rehacerlo.

1. Localizar el trail. Transcript local del workspace activo, URL de cloud-agent, o rama pusheada. No glob `~/.cursor/projects/*/`. Transcripts largos se parsan en un subagente.
2. Reconstruir estado. Rama, worktree, qué aterrizó, todos abiertos, decisiones. El trail previo es input autoritativo.
3. Diff done vs pending. Nombrar el resume point. No re-correr un repro previo ni rehacer trabajo terminado.
4. Enrutar el resto al playbook que corresponda.
5. Verificar claims heredados contra el goal original en el artefacto real. Un self-report previo passing no es proof.

### Pause safely

Dueño de una parada limpia. Explícito únicamente. "Keep going" y "going to bed, keep going" no son pause.

1. Parar en un límite seguro. Terminar el paso atómico o echar atrás. Nunca parar mid-edit en estado roto. Cancelar subagentes anidados.
2. Ninguna acción irreversible para pausar. Ni PR ni push salvo que ya hubiera uno fuera.
3. Hacer el trabajo durable. Un commit `wip:` claro. Si el tree está roto, decirlo en el body.
4. Nota de resume fuera de contexto. Intent, progreso, estado, next steps, archivos clave, gotchas. Si ya hay trail de show-me-your-work, apuntar a él.

Respuesta. Dónde en el loop, qué está en disco vs en la cabeza, commits, si el tree está limpio, primera acción al resume. Es una pausa, no el reporte final.

### Multi-phase plan

Dueño del plan, no del código. No implementar.

1. Cambio de uno o dos archivos con enfoque obvio, skip plan.
2. Zanjar preguntas abiertas con Prototype. Preguntar al operador solo una llamada de producto que ninguna corrida zanja.
3. Explorar en subagentes `poteto-agent`. Devuelven punteros, no dumps.
4. Copiar el skeleton al archivo de plan. Una sección por PR. Una unidad de cambio con su propia evidencia. Nombrar el playbook de ejecución. Autopilot-full vs stack según la regla de autopilot-stack. Programa standing → Orchestrate.
5. Escribir con **technical-writing** (modo how-to) y **unslop**.
6. Correr `node pstack/skills/poteto-mode/scripts/check-plan.mjs <plan.md>` y arreglar cada línea.
7. Devolver path y output del script. Parar. La ejecución empieza con go explícito.

Tests solos no son verificación suficiente. Un PR está verificado cuando unit, live y perf están checked. Live es mandatory. Diez lanes en Grok en el head del PR, superficie real. Una lane es Regression contra trunk. Perf es dual-sided, trunk y head. Un PR que cambia interacción es review-gated.

Skeleton. How to read this. Program checklist (Arm, Spawn owners, PR mechanics, Verdict and merge, Boot recipe). Una sección por PR (Depends on, Files, Build, You see, Verify unit/live/perf, Review gate, Merge). Close the program. Apéndices A prototipos, B alternativas, C riesgos, D links.

### Worktree cleanup

Dueño del disco y del safety gate. Borrar es irreversible.

1. Snapshot `df -h /`. Correr `scripts/worktree-audit.sh`. Clasifica por tamaño, edad, merge, uncommitted, PR, chat más reciente.
2. El bucket es consejo, no permiso. Chats pinned o activos ganan.
3. Verificar uso antes de borrar. Fan-out a transcripts.
4. Pausar ante pérdida irreversible. `wip:N` se muestra y se decide. Clean + merged + not-in-use procede.
5. Prune del set confirmado. `git worktree remove --force`, luego `git worktree prune`. Las refs de rama sobreviven.
6. Simuladores iOS y otros reclaimers (DerivedData, backups de Cursor, caches de paquetes) solo si el usuario no pidió conservarlos.

Respuesta. `df` before y after, espacio reclamado, worktrees pruned, razón de una línea por cada uno retenido.

### Opening a PR

Se invoca al final de cada otro playbook.

Trabajar desde un worktree off main. Varios Task en la misma rama, cada uno su worktree, o `git fetch && git reset --hard origin/<branch>` entre ellos. Commit liberally. Rebase en commits chicos ordenados antes de los PRs. Cada commit es un PR futuro. Amend cuando el fix pertenece al commit recién hecho.

Higiene. `/deslop` sobre el diff antes de commit. `/no-comments` antes de review. Título, descripción y body de commit por **technical-writing** (todas las capas excepto Diátaxis) y luego **unslop**.

Títulos Conventional Commits. `type(scope): subject`. Types. `feat`, `fix`, `docs`, `refactor`, `test`, `chore`, `perf`. Scope = área cambiada. Subject corto e imperativo. Nombrar un símbolo real cuando carga el cambio. Sin punto final.

Descripción, en este orden, omitir vacías.

1. `## Why`. Intent y enfoque, 1 o 2 párrafos cortos. Sin SHAs ni genealogía de rebase.
2. `## Scope`. Bullets de símbolos y paths reales. Ambos lados de un rename.
3. `## Tradeoffs`. Solo alternativas rechazadas que un reviewer preguntaría. Skip si no hubo elección real.
4. `## Blast Radius`. 1 a 3 oraciones. Quién o qué se tocó. Safe o risky. Costo si main se queda rojo.
5. `## Verification`. Cada path real corrido y su outcome. Perf, un número primario `before → after` con unidad. El resto de evidencia se linkea.

Después, videos o screenshots cuando prueban un claim. No pegar SHAs completos, recitals de swarm o arena, ni boilerplate de `## Summary` o `## Test plan`.

Forge. `gh` default. Origin si resuelve. No exigir `gt`. Preferir cinco PRs estrechos a uno gordo. Stack = cadena de base-branch. Root → trunk. Child rebasea al tip exacto del parent y el PR apunta a la rama del parent. Todo PR se abre ready, never draft. Abrir un PR no empieza babysit. Un subagente que abre PR corre interrogate, deslop y no-comments, devuelve la URL y no babysit.

## Triage de Bugbot

Clasificar cada hilo antes de actuar.

| Clasificación | Cuándo |
|---|---|
| `fix` | Problema plausible de corrección, seguridad, privacidad, pérdida de datos, auth, billing, migración, idempotencia, race, o comportamiento ya enviado. Arreglar en el PR dueño más bajo. |
| `dismiss` | Patrón documentado de bajo riesgo y el código prueba que no hace falta cambio. Responder con razón breve. |
| `ask` | Novedoso, alta severidad, seguridad o datos, o ambiguo. Ante la duda, ask. |

Patrones recurrentes de skip. Cambios visuales intencionales. Uso upstack que Bugbot no ve. Duplicación temporal durante implementación paralela. Invariante de framework que ya cubre la advertencia. Follow-up diferido declarado por el owner. Comentarios auto-retirados o falso positivo explícito.

Nunca auto-skip. Seguridad, privacidad, auth, billing, retención, permisos, alta severidad, migración, schema, idempotencia, concurrencia. Reimplementaciones manuales de comportamiento nativo del browser casi nunca se skipean. Drift de contract-test se verifica corriendo el test primero.

Desde el tercer pase, inclinarse a dismiss patrones documentados. Seguir escalando seguridad, auth, billing, datos y migraciones.

## Skills situacionales

`/poteto-mode` las corre cuando un paso las necesita. También se pueden invocar directo.

### how

Walkthrough de cómo funciona un subsistema. Nivel de onboarding senior.

Simple (módulo único, pregunta estrecha). Un explainer readonly en Fable. Complex (cross-cutting). 2 a 4 explorers readonly en Grok, luego un synthesizer en Fable. En duda, camino simple.

Salida. Overview, Key Concepts, How It Works, Where Things Live, Gotchas. No adivinar desde nombres. Leer código real.

### why

Motivación e intent. Detective de caso frío.

Anchor de código primero (`git blame`, `git log`, `gh pr view`). Luego investigators en paralelo, uno por categoría de evidencia que los MCPs expongan. Source control siempre. Issue tracker, docs largos, chat, observabilidad, error tracking, analytics. Investigators en Grok, agent mode (readonly quita MCP). Synthesizer en Fable.

Epistemics. Direct, Supported, Inferred, Speculative, Unknown. Un null result se reporta. "Nadie escribió por qué" es una respuesta. No citar código como evidencia de su propio intent.

Si la pregunta precede un cambio, convertir findings en constraints Preserve / Change / Avoid / Risk.

### recall

Brief de estado actual desde tu chat history y el registro compartido.

Un chat específico es Session pickup, no recall. Hábitos que deberían ser skill van a automate-me. Default, ventana de 7 días. Nunca leer transcripts de otro proyecto sin permiso. Mining en paralelo, ordenar por `ls -t`. Verificar estado live con git y gh.

Salida. Capsule de hasta 5 bullets. Threads con tags exactos (`[merged #N]`, `[open PR #N]`, `[in flight <branch>]`, `[verified, uncommitted]`, `[reverted #N]`, `[planned, not started]`). Problems recurrentes. Un next move concreto.

### teach

how + why tejidos en una explicación llana. No cambia nada. Definición → caso → how → why → edges. Diagramas incrementales, no un mega-diagrama. La respuesta es la explicación, no un reporte de proceso.

### blast-radius

Qué más podría romper un cambio que se ve chico, y probar el hecho de seguridad central corriendo código.

Escala de certeza. 1 dijiste que sí (inútil). 2 apuntaste a una línea. 3 mostraste que el mal caso no puede ocurrir. 4 lo corriste. 5 lo reproduciste en la app corriendo. Step 4 es el mínimo para safety facts. Cambio ancho se corre como arena.

### architect

Asentar usage, types y forma de módulos antes de implementar.

- Phase A. Ground con how. why si redefine ownership.
- Phase B. arena de sketches. Mínimo 2 candidatos estructuralmente distintos. Screen contra red flags. Módulos shallow, information leakage, descomposición temporal, pass-throughs. Preferir profundidad de interfaz.
- Phase C. Checkpoint solo si lo pides (`/architect with checkpoint`).
- Phase D. Implementar contra el sketch. Desviaciones son señal.
- Phase E. Scrap cuando el diseño está mal. Re-how, redesign from first principles, subtract, re-arena.

Default, sigue a implementación. El sketch es contrato.

### arena

N intentos paralelos del mismo brief. Cada candidato en su worktree o directorio. Un judge readonly de otra familia de modelo puntúa. El coordinador lee cada candidato end to end, elige base, graft las mejores ideas de los perdedores y verifica. Tie → cleaner boundary (Laziness Protocol). Convergencia total, shippear el consenso, no graft. Divergencia salvaje, reframe y re-run.

### swarm

N workers en slices, matrices, gauntlets o brazos de carrera. Cada uno reporta `PASS`, `ISSUES` o `BLOCKED`. El padre agrega un reporte. Default cloud. Local solo si necesita esta máquina.

Arena da a todos el mismo brief y luego selecciona y graft. Swarm cubre slices o corre una carrera con la regla de selección declarada al frente (`first pass`, `rank all`, `best-of`). Sin ceremonia de base y graft.

### interrogate

Varios reviewers en familias distintas desafían el mismo diff, intent y rúbrica. Consenso de 2+ modelos es señal alta. El lead (el padre) ordena en Act on, Consider, Noted, Dismissed, con razón de cada dismiss. No aplica nada automáticamente. Act on, idealmente 5 o menos. Lentes. Correctness, root causes, integridad estructural, verificación, complexity budget, security. Lee también los dismissals. El lead no es oráculo.

### tdd

Solo cuando el path de test es barato y local, o el usuario lo pide.

Entender el bug. Elegir el check ejecutable más estrecho. Escribir el test que falla primero. Correrlo y confirmar que falla por la razón intentada. Fix más pequeño. Rerun. Validación cercana. Si es impráctico, explicar por qué y usar el check ejecutable más cercano. Preferir ningún test a un mal test.

### no-comments y Comment Sicko

Spawn `subagent_type: "Comment Sicko"`. Es un reviewer de solo lectura, obcecado con borrar comentarios. Conserva headers legales, doc comments de API pública, links a dependencias externas no obvias, prettier-ignore, suppressions de lint válidas, links a issue o RFC de constraint. MUST KILL sorpresas del código propio. Duda, el comentario muere.

El padre inspecciona el reporte, rechaza escapes de scope, aplica fixes, y ofrece encodings (tipo, runtime, test, CI) para comentarios de constraint. Approval antes de encode. Unattended necesita pre-approval.

### unslop

Siempre aplica. Corta tells de AI. Los números de regla son ids estables. Una regla borrada deja un hueco. El proceso es escanear, reescribir conservando sentido y tono, y auto-auditar "¿qué hace esto obviamente generado por AI?".

| Id | Regla |
|---|---|
| 3 | Frases -ing superficiales. highlighting, ensuring, reflecting, showcasing, fostering. Borrar o expandir con fuentes reales. |
| 5 | Atribuciones vagas. Experts believe, Industry reports suggest. Nombrar la fuente o borrar. |
| 7 | Vocabulario AI. Additionally, crucial, delve, enduring, enhance, fostering, garner, interplay, intricate, landscape abstracto, pivotal, showcase, tapestry, testament, underscore, vibrant. Palabra llana. |
| 8 | Formas fancy de "is". serves as, stands as, boasts, features. Decir is o has. |
| 9 | "Not just X, but Y." Enunciar el punto directo. |
| 10 | Rule of three forzada. Usar el número natural. |
| 11 | Ciclado de sinónimos. Elegir una palabra y repetirla. |
| 12 | Rangos falsos. from X to Y cuando X e Y no están en una escala. Listar temas. |
| 13 | Raya larga. Evitarla por completo. Punto o coma. Sin paréntesis, sin en dash, sin hyphen como sustituto. |
| 14 | Dos puntos como muleta a mitad de frase. Válidos solo antes de lista o ejemplo. |
| 15 | Bold de más. No boldar cada proper noun. |
| 16 | Listas con header inline que restata la línea. Un lead-in bold que termina en punto y aporta detalle nuevo sí vale. |
| 17 | Title case en headings. Usar sentence case. |
| 18 | Emojis decorativos. Fuera de headings y bullets. |
| 19 | Curly quotes. Straight quotes. |
| 20 | Frases de chatbot. I hope this helps, Let me know if, Of course, Certainly, Found the smoking gun. |
| 22 | Sicofancia. Great question, You're absolutely right. Responder directo. |
| 23 | Filler. In order to → To. Due to the fact that → Because. It is important to note that se borra. |
| 24 | Hedging excesivo. could potentially possibly → may. |
| 25 | Conclusiones genéricas. The future looks bright. Hechos o planes concretos. |
| 26 | Sustantivos metáfora abstractos. substrate, wedge, vector, locus, vantage, nexus, primitive como noun, harness como metáfora, surface como API surface, bedrock, scaffolding como metáfora, modality, paradigm, gold-plating, ratchet como metáfora, evacuate para mover código, endgame, north star, flywheel. Palabra concreta. |
| 27 | Decir el mecanismo o un número, no el feeling. Si la oración podría aparecer igual en docs de otro proyecto, no dice nada de este. Cortarla. |
| 28 | Una idea por oración. Si el lector tiene que volver atrás, partir. |
| 29 | Voz activa. Nombrar el actor. Pasiva solo si el actor es desconocido o no importa. |
| 30 | Cortar adverbios o usar un verbo más fuerte. significantly improves → el delta medido. |
| 31 | Palabra llana. utilize → use. leverage → use. facilitate → help. |
| 32 | Prosa amanerada. Aforismos, fragmentos retóricos, código personificado, verbos figurativos. Decir lo que se quiere decir. |
| 33 | Sobre-compresión. Artículos caídos, fragments sin verbo, symbol-speak, flechas. Oraciones enteras. |

### technical-writing

Capas. Diátaxis (un documento, un modo. Tutorial, how-to, reference, explanation). Google developer style (you, presente, commands, condición antes de instrucción). STE (una instrucción por oración, ~20 o 25 palabras, conservar artículos). Global English (colocar "only", romper noun strings, no "it/this" ambiguo). Luego unslop. Tres meta-reglas. Cortar palabras inútiles. Palabras cotidianas. Las reglas sirven al lector.

Bodies de PR. Briefing de menos de un minuto. Linkear logs, no pegarlos. El codebase es la word list. No inventar jerga.

### bro

Reformular el último mensaje en lenguaje llano, sin jerga. Una sola acción.

### typescript-best-practices

Se carga solo al tocar `.ts` o `.tsx`. Primero aplica **type-system-discipline**. Luego esta tabla.

| Regla | Qué exige |
|---|---|
| Discriminated unions | Variantes con un `kind` literal. Sin bolsas de optionals. |
| Branded types | Primitivos con `& { readonly __brand: "X" }`. Validar una vez en el límite. |
| Constructive modeling | Construir la forma para que el valor ilegal no se pueda crear. `[T, ...T[]]` no vacío. `start` más `duration` para un rango. No un guard de runtime. |
| Simplest total type | Dejar `T[]` mientras toda operación sea total. Fortalecer a `NonEmpty<T>` solo donde el tipo flojo fuerza `!`, un cast o un throw de "should never happen". |
| `unknown` over `any` | Dato externo es `unknown`. |
| Schemas before guards | Usar la librería de schemas del repo e inferir el tipo (`z.infer`) antes de un type guard a mano. |
| No `as` | Cast solo después de validar. |
| Narrowing hierarchy | Discriminant switch > `in` > `typeof`/`instanceof` > user-defined type guard > `as`. |
| Type guards | Deben verificar el claim. Un guard que miente es peor que `as`. Nombres `isX` o `hasX`. |
| Exhaustiveness | `const _exhaustive: never = x` en el default. |
| `satisfies` over `as` | Valida el valor sin widening de literales. |
| Boundary validation | Parsear al cruzar, a un tipo de dominio nombrado. `Record<string, unknown>` se acaba ahí. |
| Schema-derived types | `Pick` / `Omit` / `Parameters` / `ReturnType` / `Awaited` / `typeof` antes de un interface nuevo. |
| Object args | Objetos, no posicionales, salvo hot paths. |
| Real tests | No mockear lo que puedes correr. UI en un build corriendo. |
| Structured telemetry | Logger con contexto suficiente para debuggear desde un id. No `console.log` shipped. |

### figure-it-out

Cuando ningún playbook bundled encaja, o el trabajo es grande, o el usuario lo dejará para confiar después.

- A. Frame. Predicado falsifiable, scope cuantificado, rigor sesgado alto.
- B. Diseñar el workflow. Unidades landable independientes, riskiest-first, harness de verificación antes del trabajo, architect en one-way doors, worktrees separados.
- C. Loop. Hipótesis → cambio más pequeño → medir en el artefacto real → keep o revert. Veredictos VERIFIED, NOT VERIFIED, INCONCLUSIVE. Inconclusive no es pass.
- D. Trail con show-me-your-work.
- E. Verificar el todo contra el predicado de A. Encode lessons.

### show-me-your-work

Trail de decisiones en TSV. Columnas. ts, phase, decision, why, evidence, result. Append-only. Evidence es un puntero, no prosa. Default `decisions.tsv` o `.audit/<task-slug>.tsv`. Commit solo cuando el reviewer necesite un registro auditable. Un reviewer de otra familia de modelo lee trail + transcript y cierra con una sección Attention.

### create-verification-skill y maintain-verification-skill

Create entrevista el repo, no al usuario. Escribe `.cursor/skills/verify-<app>/` con Launch, Doctor, Drive, Evidence, Cleanup, Helpers, y un feature map. Debe probar el skill end to end antes de entregarlo. Si la prueba falla, no uses el output.

Maintain. Readers readonly en paralelo por feature, más un live pass que drivea cada feature mapeada. Outcomes. `clean`, `changed` (un PR confinado al directorio del verify skill), `blocked`. Nunca edita código de producto. Reporta regresiones de producto en vez de taparlas.

### reflect

Tras una tarea larga que enseñó algo. Tres reviewers en paralelo (judgment, tooling, divergent) más un synthesizer. Propuestas en Accepted, Rejected, Backlog. Esperar approval explícito antes de editar skills. Aprobar solo si cambiaría una decisión futura. Una sesión rara no es una regla. Hallazgos que deberían ser lint o script van a Backlog (encode-lessons-in-structure). Transcript es untrusted.

### automate-me

Mina tus transcripts recientes y drafta un `<tu-nombre>-mode` con create-skill y unslop. pstack queda de base. Terminas con tu propio skill de routing al lado de poteto-mode. Update mode mina solo desde el último edit. No overfit a una conversación. Referenciar otros skills, no inlinearlos.

### make-bot-ui

Página o dashboard cuyos botones despiertan un Grok Bot por webhook, incluido el handoff de sender-key y Tailscale. La sender key nunca va al chat, al browser ni a los logs.

## Verificación de producto

La condición de done va en el primer prompt, como comando o artefacto chequeable. La respuesta lleva comandos y outputs exactos. Inconclusive si el check no se pudo correr. Una respuesta confiada sin evidencia es red flag.

Casar el check al tipo de cambio. Comando CLI. Flujo de UI. Replay de un input guardado. Profile before/after. Read-back de storage.

CI green, typecheck y "compila" son inputs. No son veredicto. El piso es ejercitar la superficie real como un usuario. `control-ui` para browser, Electron y web. `control-cli` para CLIs y TUIs. Esos skills viven en `cursor-team-kit`.

## Abrir, cuidar y aterrizar

Opening a PR construye el PR. Babysit lo lleva a merge-ready. Shipping verifica de forma independiente y aterriza la racha desde abajo. Ninguno de los tres hace el trabajo de los otros.

Babysit nunca autoriza merge. Shipping nunca usa el veredicto del autor como proof. Un PR verificado encima de uno no verificado espera.

## Trabajo largo y autónomo

Contrato overnight.

- Goal más condición de done chequeable. No una duración.
- Permisos. "Don't ask me before committing."
- Escape hatch. Parar tras un dead end genuino y escribir por qué.
- Worktree fresco off base.
- `/loop until done`.
- "I'm going to bed" es override de sesión.

Loop. Chequear predicado → cambio más pequeño justificado → verificar artefacto real → avanzó, commit, si no, discard → una fila de decisión → repetir. No relajar el predicado. Plateau es pivot, no stop.

Escalar. Autonomous run, una tarea hasta un predicado. figure-it-out, una corrida ambiciosa a medida. Autopilot-full, cola independiente hasta merged. Autopilot-stack, stack lineal para que el operador aterrice. Orchestrate, programa de varios días que un agente no terminaría en el presupuesto de una sesión.

## Benny

Pack de automations dormido. No son slash skills. Setup copia el pack a `.cursor/automations/benny/` del repo destino, habilita pstack ahí para skills compartidas, y deja la config de usuario fuera del pack.

Automation 1, triage de reportes de Slack. Bug, perf, feature, question, reroute. Un reply en el thread con `[benny:bug]`, `[benny:performance]` o `[benny:other]`.

Automation 2, reproduce y fix. Espera el marker de triage. Repro 2 veces vía UI real. Fix acotado opcional más PR draft. TDD si es barato. Smoke de blast-radius. Nunca merge ni deploy. Fail closed si falta config.

## Lo que no viene en el plugin

`/deslop` y `deslop` viven en `cursor-team-kit`. `control-cli` y `control-ui` también. `/create-skill` es built-in de Cursor. Cursor también tiene un `/babysit` built-in. Dentro de poteto-mode, el playbook Babysit lo sustituye para pedidos de estado de PR.

Instala `cursor-team-kit` al lado de pstack si quieres el set completo.

pstack no shippea skills de planning. Cursor ya tiene Plan mode. La mejor spec es código. Si quieres un plan, poteto-mode lo cubre. No es el default.

## Recetas

```text
/how do we cancel runs? do we have an n+1 when we look up every run to cancel?
/why was the retry limit set to five? does the reason still hold?
/teach me how this PR changes retries. convince me it fixes the cause.
/recall catch me up on the export work from last week
/architect design the import pipeline before writing any code
/arena take my prompt to the arena verbatim
/swarm check every package under packages/ against its check.sh. one worker per package
/interrogate the whole branch, but skeptically
/tdd implement
/blast-radius what else could this break
/unslop tighten the new changes
/bro
/reflect that took too long. capture what we learned
/show-me-your-work keep a decision trail
/automate-me
/setup-pstack
```

Prompts de poteto-mode que ya enrutan.

```text
/poteto-mode this pr has a subtle bug where the scroll drifts every 750ms even when idle. repro first, then fix and verify.
/poteto-mode a big list takes a second or two to load. run a cpu trace and tell me why.
/poteto-mode build a small feature behind a feature flag. verify it really works.
/poteto-mode build two prototypes of the markdown renderer so we can compare.
/poteto-mode i'm going to bed. land the stack even if ci flakes.
/poteto-mode check on pr 123. anything outstanding?
/poteto-mode the row spacing is too tall when this flag is on. the second image is correct.
/poteto-mode i'm stepping away. migrate every caller from the sync store to the async one. keep behavior identical.
/poteto-mode what's eating my disk? prune the worktrees that are safe to prune.
```

## Errores a evitar

1. Enumerar skills en el prompt. El playbook ya las secuencia. Nombrar un skill solo para override.
2. Condición de done vaga. Dar un comando o artefacto pass/fail.
3. Varios agentes en el mismo worktree. Un worktree por intento.
4. Usar arena para cobertura. Arena es un brief, varios diseños. Swarm es slices particionados.
5. Aceptar cada comentario de review. Interrogate ordena act-on vs dismissed.
6. Tratar `auto` como slug de modelo. Significa omitir `model` para heredar el padre.
7. Declarar success por un build verde. Pedir comando, flujo, valor o profile real más evidencia.
8. Escribir un SKILL.md a mano. Pasar por el playbook de authoring.
9. Editar un skill a mitad de tarea. Arreglarlo en su propio PR.
10. Relajar el predicado overnight para declarar victoria. Duración ("trabaja 4 horas") no es condición de done.
11. Confiar en el self-report de un subagente. Revisar el artifact.
12. Preguntar "¿hago X?" en trabajo reversible.
13. Compensar un síntoma (nil-check, shim, dual path) en vez de ir al cause o al target.
14. Name-dropear un principio sin decir qué decisión cambió.
15. Abrir un PR draft. Ready, never draft.
16. Empezar babysit al abrir el primer PR. Terminar la fase o el stack primero.
17. Mergear desde babysit. Eso es Shipping.
18. Aterrizar un PR verificado encima de uno no verificado.

## Mapa de decisión rápido

```text
¿Es una pregunta y no quieres código?
  → Investigation, how, why, teach, recall

¿Hay un defecto?
  → Bug fix. Repro primero. tdd si el test es barato.

¿Hay lentitud medida?
  → Perf issue si es un fix. Hillclimb si es un loop contra un target.

¿Hay un síntoma live o un trace ya capturado?
  → Runtime forensics o Trace forensics. Diagnóstico, no fix.

¿Hay comportamiento nuevo?
  → Feature. how → architect → checkpoint → implementar → verificar → interrogate si está contestado.

¿Solo cambia la forma?
  → Refactoring. Pin primero. Subtract. Equivalence. Menor reader load o revert.

¿Hay que decidir barato?
  → Prototype. Throwaway. Luego Feature.

¿Cruza un límite de función?
  → architect, que trae arena.

¿Quieres N intentos del mismo brief?
  → arena.

¿Quieres cobertura o una carrera?
  → swarm.

¿Quieres romper el diff?
  → interrogate.

¿Ningún playbook encaja, o te vas y quieres confiar al volver?
  → figure-it-out + show-me-your-work.

¿Es un programa de varios días?
  → Orchestrate.

¿Es una cola de PRs?
  → Autopilot-full si aterrizas. Autopilot-stack si revisas y aterrizas tú.

¿El PR ya existe?
  → Babysit hasta merge-ready. Shipping para aterrizar.
```

## Principio que gobierna el resto

Di el goal y cómo sabrás que está hecho, en tus palabras. pstack elige el playbook, corre las skills y te muestra la evidencia. El agente va profundo primero. El paralelismo viene después, cuando cada unidad ya es verificable.
