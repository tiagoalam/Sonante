# Sonante 0.5.5

Mudanças acumuladas desde a referência pública v0.4.13. As evidências e os hashes de cada artefato acompanham a entrega.

- Biblioteca local: múltiplas coleções, favoritos e playlists; validação da identidade das origens e preservação dos registros quando uma coleção fica indisponível.
- Recuperação guiada: análise do bloqueio de inicialização, oferta vinculada ao estado observado, proteção dos controles antes de executar e indicação explícita de conclusão ou incerteza. A reprodução continua sob comando do usuário.
- Disponibilidade: consulta manual por coleção, com resultados referentes ao instante da observação; o resultado não concede acesso nem autoriza reprodução.
- Navegação local e capas: trabalho limitado, reaproveitamento da prova de topologia com invalidação e fallback visual quando uma imagem falha ao decodificar.
- Inicialização: mensagens específicas para formato de biblioteca incompatível, configuração inválida e transação incompleta; reabertura preserva dados e não inicia reprodução automaticamente.
- Áudio e sessão: MPD privado, modos Shared e Direct, controles de EQ/ganho/mute e Signal Path; preservação de pausa/parada e guardas de lifecycle. A integração Plex permanece preservada.
- Distribuição Linux x86_64: pacote completo com motores e recursos fixados, fontes correspondentes e hashes por formato. Fedora usa seu manifesto de compatibilidade próprio.

A versão não afirma fidelidade bit-perfect nem compatibilidade universal de dispositivos e codecs. Resultados de testes se referem às versões, pacotes, ambientes e rotas identificados nos recibos acompanhantes.
