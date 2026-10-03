# AGENTS.md — Sonante

Este arquivo fornece contexto persistente para agentes que trabalhem neste repositório. Ele se aplica a toda a árvore do projeto.

## 1. Regras permanentes do projeto

Estas regras expressam prioridades e invariantes do Sonante. Elas devem continuar válidas mesmo quando arquivos, versões, comandos ou detalhes internos mudarem.

### Prioridade fundamental

- Estabilidade, previsibilidade e fidelidade da reprodução têm prioridade sobre novas funcionalidades, conveniência de UI e velocidade de entrega.
- Mudanças no caminho de áudio devem falhar de forma explícita e segura. Manter a saída anterior funcionando é preferível a aceitar parcialmente uma nova configuração.
- Não ampliar o escopo de uma mudança de áudio sem antes compreender o ciclo completo React → Tauri → Rust → MPD → ALSA.

### Segurança do repositório e escopo de mudanças

- Execute `git status --short` antes de qualquer alteração para identificar e preservar mudanças preexistentes do usuário.
- Nunca execute `git reset --hard`, `git clean` ou outra operação destrutiva sem autorização explícita do usuário e confirmação exata dos alvos. Não descarte, sobrescreva ou reverta trabalho preexistente para facilitar uma implementação.
- Não modifique, reformate, renomeie nem reorganize arquivos fora do escopo solicitado. Evite formatadores ou correções automáticas com alcance maior que os arquivos autorizados.
- Ao precisar tocar um arquivo que já contém mudanças não relacionadas, preserve-as e limite o patch às linhas indispensáveis para a tarefa.

### MPD, estado e erros

- Nunca ignore silenciosamente erros relacionados ao MPD, incluindo inicialização, conexão, comandos, ACKs, timeouts, restauração de fila, rescan, shutdown ou troca de saída. Propague um erro estruturado até a UI, registre contexto suficiente e preserve/recupere o último estado válido quando possível.
- Nunca persista nem publique como confirmado um novo estado antes de a operação correspondente ser aceita pelo componente que é sua fonte de verdade. Exemplos:
  - não substitua/persista a fila espelhada antes de o MPD aceitar `clear`/`add`/`play`;
  - não salve uma nova saída como ativa antes de o novo MPD iniciar, responder e validar a saída;
  - não mostre sucesso na UI antes da confirmação do backend.
- Operações compostas, sobretudo troca de dispositivo, devem ser transacionais: capturar estado, preparar, aplicar, validar, restaurar e confirmar. Em falha, fazer rollback ou retornar um estado de falha explícito; nunca continuar como se a operação tivesse sido bem-sucedida.
- Trate nomes de arquivos, paths, tags e URIs locais/remotos como dados não confiáveis ao construir comandos MPD. Use escaping compatível com o protocolo, rejeite CR/LF quando necessário e nunca concatene argumentos crus dentro de command lists.
- Diferencie rigorosamente EOF, timeout, erro de transporte, resposta parcial, `ACK` do MPD e `OK`. Uma leitura interrompida não é sucesso.
- Valide índices da fila, estados, URLs, opções de configuração e dispositivos no backend; tipos TypeScript não são uma fronteira de segurança.
- O processo MPD deve ser encerrado somente por um handle/identidade cuja propriedade tenha sido comprovada. Nunca envie sinais a um PID de arquivo stale sem validar que ele pertence à instância do Sonante.
- Use socket e arquivos de runtime com escopo por usuário/instância, preferencialmente sob `$XDG_RUNTIME_DIR`; evite nomes globais previsíveis em `/tmp`.
- Nunca exponha tokens Plex, credenciais ou outros segredos em logs, mensagens de erro, debug output, commits, diffs, relatórios ou respostas ao usuário. Remova/redija tokens e parâmetros sensíveis de URLs antes de registrá-los ou exibi-los, inclusive `X-Plex-Token` em query strings.

### Concorrência e recursos

- Não mantenha `Mutex`/locks durante rede, espera de processo, leitura grande de disco, diálogo nativo ou outro I/O potencialmente demorado quando for possível copiar um snapshot e liberar o lock antes da operação.
- Defina uma ordem de aquisição quando mais de um lock for indispensável e evite caminhos que possam adquirir os mesmos locks em ordem diferente.
- Não use polling duplicado para a mesma telemetria. Prefira uma fonte única de estado no frontend e, quando viável, eventos ou o mecanismo `idle` do MPD.
- Cancele ou invalide resultados assíncronos obsoletos. Uma resposta antiga de busca, autenticação ou navegação não pode sobrescrever uma solicitação mais recente.
- Todo processo, socket, stream, timer, observer e request de longa duração deve ter ciclo de vida e cleanup explícitos.

### PCM, DSD, DoP e alegações de fidelidade

- PCM em modo Exclusive deve chegar a um endpoint ALSA de hardware sem mixer, DSP, ReplayGain ou resampling no caminho quando a reprodução for apresentada como bit-perfect. Se o formato não for suportado pelo DAC, retorne erro claro; não converta silenciosamente.
- DSD nativo e DoP são caminhos diferentes. Não use os termos como sinônimos e não infira o caminho efetivo apenas pela extensão do arquivo ou pelo formato de entrada informado pelo MPD.
- DoP só deve ser habilitado para uma saída Exclusive compatível, com suporte comprovado do MPD/ALSA/DAC. Shared não deve ser apresentado como DoP ou DSD nativo sem validação específica do caminho efetivo.
- Volume por software, ReplayGain, crossfade, equalização, conversão DSD→PCM e qualquer DSP alteram as condições necessárias para uma alegação bit-perfect. A UI deve refletir isso imediatamente.
- Nunca afirme nem exiba “bit-perfect” apenas porque a saída é `hw:*` ou porque existe um `audio_format`. A alegação exige, no mínimo: endpoint direto conhecido, formato aceito sem conversão, mixer/DSP/ReplayGain desativados, volume não destrutivo e confirmação coerente do caminho de saída.
- Para Shared, assuma que PipeWire, PulseAudio, dmix ou a configuração ALSA do sistema pode misturar ou reamostrar. Descreva-o como compartilhado, não como bit-perfect.
- Toda alteração que possa afetar PCM/DSD/DoP, buffer, seek, gapless, fila, troca de sample rate ou abertura/fechamento do DAC exige testes proporcionais antes de ser considerada concluída.

### Compatibilidade de fontes

- Preserve compatibilidade tanto com biblioteca local quanto com Plex. Uma correção de fila, metadata, duração, cover, navegação ou reprodução deve considerar os dois tipos de URI.
- Não presuma que uma URI HTTP é sempre Plex nem que uma path local é sempre biblioteca local: o modo legado de mapeamento Plex pode produzir paths locais.
- Mantenha metadata em um modelo comum (`TrackMetadata` ou sucessor), mas preserve identificadores e informações específicas da fonte quando forem necessárias para navegação ou retomada.
- Bibliotecas locais podem conter aspas, Unicode, novas linhas inválidas, symlinks, roots sobrepostos, discos removíveis, álbuns homônimos e I/O lento. Identidade de álbum não deve depender apenas de artista+título.
- Plex pode estar offline, responder 401/404/5xx, atrasar, mudar de rota ou interromper um stream. Verifique status HTTP e apresente erros úteis; lista vazia não deve mascarar falha de autenticação/rede.

### Testes obrigatórios para o caminho de áudio

Antes de alterar supervisor, transporte MPD, fila, configuração de saída ou geração de `mpd.conf`:

- Quando o comportamento existente não tiver cobertura, adicione primeiro testes de caracterização que registrem o comportamento observado antes de refatorá-lo. Diferencie nesses testes o comportamento intencional de bugs conhecidos que não devem ser perpetuados.

1. adicione ou atualize testes unitários para escaping/parsing, transições de estado e rollback;
2. teste falhas: MPD ausente, socket atrasado, `ACK`, timeout, dispositivo ocupado, DAC removido e restauração recusada;
3. teste fila vazia, parada, pausada e tocando, incluindo preservação de índice e posição;
4. teste arquivos locais e streams Plex, inclusive paths/URIs com espaços, Unicode, aspas e barras invertidas;
5. para mudanças de formato, cubra PCM 16/44.1, PCM hi-res suportado e não suportado, DSD nativo quando disponível e DoP habilitado/desabilitado;
6. valide que Shared continua coexistindo com outros aplicativos e que Exclusive libera o DAC em stop, troca, falha e encerramento;
7. registre limitações quando hardware real não estiver disponível. Teste simulado não autoriza sozinho uma alegação de bit-perfect.

Não use somente a ausência de erro de compilação como validação do caminho de áudio.

## 2. Arquitetura real — retrato mutável

Esta seção descreve o estado observado na versão 0.4.0. Ela pode ficar desatualizada; confirme no código antes de agir e atualize esta seção quando a arquitetura mudar materialmente. As regras da seção 1 continuam valendo.

### Visão geral

O Sonante é uma aplicação desktop Tauri para Linux:

```text
React/TypeScript
    │ invoke (Tauri IPC)
    ▼
Comandos em src-tauri/src/lib.rs
    │
    ├── AudioEngine ── protocolo MPD por UnixStream
    ├── AudioAnalyzer ── FIFO PCM opcional para RMS/peak e espectro FFT
    ├── MpdSupervisor ── processo/configuração MPD
    ├── PlexClient ── HTTP Plex
    ├── AppConfig ── config.json
    └── FavoriteAlbum ── favorites.json
                         │
                         ▼
                    MPD dedicado
                    ├── plugin ALSA ──► hw:* (ALSA Direct) ou default (Shared)
                    └── plugin fifo ──► analyzer Rust, somente enquanto Now Playing solicita
```

O backend Rust não envia amostras diretamente ao ALSA. O MPD externo é o motor real de streaming, decodificação, fila e saída de áudio.

### Responsabilidades atuais

- **Frontend (`src/`)**: React, navegação, biblioteca local/Plex, favoritos, configurações, onboarding, player e fila. Os serviços em `src/services/` são fachadas finas sobre `invoke`; os contratos ficam em `src/types/`.
- **Composição Tauri (`src-tauri/src/lib.rs`)**: registra comandos IPC e mantém `AudioState`, `AnalyzerState`, `SupervisorState`, `ConfigState`, `ConfigTransactionState` e `PlexState` em `Mutex`.
- **`AudioEngine` (`src-tauri/src/audio.rs`)**: cliente síncrono do protocolo MPD, fila espelhada em memória, `queue_cache.json`, status, seek/volume, listagem da biblioteca local e resolução de covers. Atualmente reúne responsabilidades que podem ser separadas no futuro.
- **`AudioAnalyzer` (`src-tauri/src/analyzer.rs`)**: consumidor opcional do output FIFO `Sonante Analyzer`, calcula RMS/peak estéreo e um espectro FFT mono combinado no backend e emite somente frames normalizados para a janela Now Playing. O supervisor cria o FIFO estável antes do spawn do MPD e só o remove após o processo encerrar; o analyzer apenas abre/fecha seu descritor. O FIFO usa `48000:16:2`, começa desabilitado e permanece inativo para DSD/DoP.
- **`MpdSupervisor` (`src-tauri/src/supervisor.rs`)**: sincroniza a biblioteca virtual de symlinks, gera `mpd.conf`, inicia/para o processo MPD e administra socket/PID/FIFO. Mudanças aqui têm impacto direto na disponibilidade do DAC.
- **`PlexClient` (`src-tauri/src/plex.rs`)**: OAuth PIN, consultas a bibliotecas/álbuns/artistas/coleções, parsing de tracks e geração de URIs HTTP ou paths mapeados. Seus clones compartilham um manager que usa `machineIdentifier` como identidade, valida/redescobre rotas Plex transitórias e não mantém locks durante HTTP.
- **Persistência**: `config.json`, `favorites.json`, `queue_cache.json`, banco/configuração do MPD e diretório virtual ficam sob o diretório de configuração do Sonante. O socket, o PID e o FIFO do analyzer ficam em `$XDG_RUNTIME_DIR/sonante/`; quando esse diretório não está disponível, o fallback privado é o subdiretório `runtime/sonante` da configuração do Sonante.

### Fluxo de reprodução atual

1. React obtém itens locais pelo MPD ou itens remotos pelo `PlexClient`.
2. A view converte a seleção em `TrackMetadata[]` e chama `audioService.playTracks`; faixas Plex carregam uma referência estável com servidor e part key, não a URL autenticada de stream.
3. Tauri resolve referências Plex para URIs efêmeras com a rota/credencial atual antes de bloquear `AudioState`.
4. `AudioEngine` resolve covers, valida e escapa os argumentos, envia `clear`/`add`/`play` ao socket MPD e só então publica/persiste a fila lógica após a aceitação do MPD.
5. Para local, MPD lê a path sob seu `music_directory`; para Plex, MPD abre a URI HTTP com token.
6. MPD decodifica PCM/DSD e usa seu plugin ALSA; quando solicitado pela janela Now Playing em PCM, um output FIFO separado alimenta o analyzer sem alterar a configuração do output principal.
7. O frontend consulta status por IPC e exibe metadata, posição, volume e formato reportado pelo MPD.

### Shared e ALSA Direct atuais

- **ALSA Direct**: `audio_output_type == "alsa"`; gera saída MPD ALSA para `alsa_device`, normalmente `hw:CARD=...,DEV=...`, e inclui `dop yes/no`.
- **Shared**: `audio_output_type == "pipewire"`/`"shared"`, ou dispositivo `default`; ainda usa `type "alsa"`, mas aponta para `device "default"`. A coexistência com o sistema depende da configuração ALSA/PipeWire/PulseAudio/dmix do host.
- Shared configurado como PipeWire usa `wpctl` opcionalmente para ler e alterar `@DEFAULT_AUDIO_SINK@` quando a sondagem inicial é válida; nesse caso o MPD gera `mixer_type "none"`. Sem confirmação segura, Shared preserva `mixer_type "software"`. Falhas posteriores do `wpctl` tornam o volume temporariamente indisponível, sem fallback silencioso. Em ALSA Direct, o Sonante associa `hw:CARD=...,DEV=...` à placa `hw:CARD=...` e usa `mixer_type "hardware"` somente quando encontra exatamente um controle ALSA legível de volume de reprodução, com canais e faixa válidos; ausência, falha ou ambiguidade preserva o mixer por software. A UI não apresenta ALSA Direct como bit-perfect, e o formato reportado pelo MPD não valida o caminho efetivamente entregue ao ALSA/DAC.

### Estado conhecido que merece cautela

- A fila em `queue_cache.json` é metadata espelhada; ela não é automaticamente restaurada no MPD no startup.
- O transporte MPD valida e escapa argumentos textuais, rejeita NUL/CR/LF e diferencia `OK`, `ACK`, EOF e erros de I/O/timeout; novos comandos devem reutilizar essas mesmas fronteiras.
- A troca de saída é serializada e transacional: captura um snapshot explícito, aplica a nova configuração e tenta rollback em falha. Snapshots Playing e Paused terminam pausados após a troca; Stopped permanece parado.
- Há polling de status duplicado no frontend.
- A biblioteca local agrega roots por symlinks e usa o índice do MPD; roots sobrepostos ou álbuns homônimos exigem cuidado.
- Streams e artwork Plex usam referências estáveis no frontend e na persistência. O token é resolvido apenas no backend; entradas legadas autenticadas são migradas quando há identidade segura do servidor ou têm somente o artwork inseguro descartado.
- Há testes unitários Rust para protocolo MPD, escaping, restauração de fila/estado, rollback e lifecycle do supervisor. Eles usam simulações e não comprovam, sozinhos, integração real com MPD/ALSA, hardware ou bit-perfect.

## 3. Convenções atuais — retrato mutável

- TypeScript estrito, componentes funcionais React e hooks.
- Estilos majoritariamente por classes utilitárias Tailwind diretamente no JSX.
- UI internacionalizada com `react-i18next`; recursos em `src/locales/en.json` e `src/locales/pt-BR.json`. Novas strings visíveis devem ser adicionadas aos dois idiomas.
- Serviços de IPC separados em `src/services/audio.ts`, `config.ts`, `favorites.ts` e `plex.ts`.
- Tipos de frontend separados em `src/types/`; nomes serializados seguem os campos `snake_case` do Rust.
- Backend dividido em módulos Rust pequenos, mas `audio.rs`, `plex.rs` e `lib.rs` ainda concentram lógica.
- Comandos Tauri retornam `Result<T, String>` no código atual. Ao evoluir, prefira erros estruturados/serializáveis sem perder mensagens úteis para a UI.
- Configuração e favoritos usam JSON legível. Preserve migração de configurações antigas ao alterar schemas.
- Estado compartilhado Rust usa `std::sync::Mutex`; trate poisoning sem `unwrap()` em caminhos de produção quando refatorar.
- Não inclua artefatos de build, caches ou pacotes gerados em mudanças de fonte salvo quando a tarefa for explicitamente de release/empacotamento.
- Preserve mudanças preexistentes no worktree e limite cada alteração ao escopo solicitado.

## 4. Comandos disponíveis — retrato mutável

Confirme `package.json` e `Cargo.toml` antes de usar. Na versão 0.4.0:

### Frontend

```bash
npm install
npm run dev
npm run build
npm run preview
./node_modules/.bin/tsc --noEmit
```

- `npm run build` executa `tsc && vite build` e escreve em `dist/`.
- Não há script de lint configurado e não há ESLint/Biome configurado. Não alegue que lint passou sem adicionar/usar uma ferramenta acordada.
- Não há script de teste frontend nem framework de testes instalado.

### Tauri/Rust

```bash
npm run tauri dev
npm run tauri build
cargo check --manifest-path src-tauri/Cargo.toml
cargo test --manifest-path src-tauri/Cargo.toml
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets --all-features
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
```

- `npm run tauri dev` inicia Vite e compila/executa o app Tauri.
- `npm run tauri build` executa o build frontend e gera bundles em `src-tauri/target/release/bundle/`.
- `cargo test` executa testes unitários Rust do projeto para protocolo MPD, restauração, rollback e lifecycle do supervisor; sucesso não substitui validação de integração com MPD/ALSA ou hardware real.
- `cargo clippy` e `cargo fmt` dependem dos componentes Rust correspondentes estarem instalados; não existem scripts do repositório que os encapsulem.
- Build, check, clippy e testes Rust escrevem em `src-tauri/target/`. Respeite tarefas explicitamente somente leitura.

### Empacotamento

- Tauri está configurado com `bundle.targets = "all"`.
- `sonante-arch/PKGBUILD` compila a tag correspondente sem gerar bundles Tauri e instala diretamente o binário, o desktop file e os ícones no pacote Arch.
- Dependências externas de runtime incluem pelo menos MPD, `aplay` e `xdg-open`; `wpctl` é opcional para o controle aprimorado de volume Shared. Valide pacotes limpos, não apenas máquinas de desenvolvimento.

## 5. Checklist de entrega

Ao concluir uma mudança:

- revise o diff completo de todos os arquivos alterados antes da entrega, procurando mudanças acidentais, reformatação fora do escopo, segredos e artefatos gerados;
- relate explicitamente quais arquivos e quais comportamentos foram alterados;
- informe quais comandos de verificação foram realmente executados;
- diferencie testes automatizados, simulações e validação em hardware real;
- relate qualquer erro ignorado, fallback ou condição não testada — idealmente elimine-os antes da entrega;
- confirme que fila, posição, estado e DAC são preservados ou liberados conforme o cenário;
- confirme comportamento para local e Plex quando a mudança tocar modelos compartilhados;
- não atualize documentação com alegações de áudio que o código e os testes não comprovem;
- se uma informação mutável deste arquivo ficar obsoleta, atualize-a na mesma mudança sem enfraquecer as regras permanentes.
