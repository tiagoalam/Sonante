// Windows-only wording for the original wizard, preferences and About page.
// The Linux locale resources and component render trees remain unchanged.
export const platformAudioText = {
 "pt-BR": {
  windowsLibrary:{errors:{windows_network_unavailable:"Não foi possível verificar a pasta de rede. Confira a conexão e o acesso de leitura no Explorador; depois atualize a biblioteca ou selecione a pasta novamente.",index_timeout:"Uma leitura da biblioteca demorou além do limite. Confira a pasta ou a conexão e tente atualizar a biblioteca."}},
  windowsPort:{outputEndpointVolume:"Volume do dispositivo no Windows: {{value}}%",outputEndpointMuted:"silenciado",outputEndpointReadOnly:"Leitura no momento da consulta. Essa consulta não altera o volume.",errors:{windows_network_unavailable:"Não foi possível verificar a pasta de rede. Confira a conexão e o acesso de leitura no Explorador; depois selecione a pasta novamente.",index_timeout:"Não foi possível concluir a leitura a tempo. Confira a mídia e tente novamente."}},
  wizard: {
   step1Desc: "Escolha Shared para a saída padrão do Windows ou Direct, em teste, para um dispositivo específico.",
   step1Shared: "O modo Shared usa a saída padrão de áudio do Windows e permite som de outros aplicativos.",
   step1Direct: "Direct solicita WASAPI exclusivo no dispositivo selecionado. A abertura exclusiva e o formato são verificados ao reproduzir.",
  },
  settings: {
   pipewireNotice: "A saída padrão do Windows pode misturar o áudio de aplicativos e converter o formato. O volume do Sonante é por software e independe do volume principal do Windows.",
   directNotice: "Direct solicita WASAPI exclusivo no dispositivo selecionado, que pode estar ocupado por outro aplicativo. Selecionar esse modo não comprova a saída física ou bit-perfect.",
   dopDesc: "WASAPI Direct tenta DoP para DSD quando o endpoint aceita PCM exclusivo de 24 bits; se não aceitar, informa erro sem converter para PCM.",
   dopOnlyDirect: "O transporte DoP para DSD está disponível somente em WASAPI Direct.",
   dopTransportNotice: "DoP transporta DSD em quadros PCM marcados; não é DSD nativo nem certificação do sinal físico.",
  },
  about: {
   audioEngineTitle: "Motor de áudio Windows",
   audioEngineDesc: "MPD com WASAPI Shared, Direct em teste e equalização opcional pelo CamillaDSP.",
  },
 },
 en: {
  windowsLibrary:{errors:{windows_network_unavailable:"The network folder could not be verified. Check the connection and read access in Explorer, then refresh the library or select the folder again.",index_timeout:"A library read exceeded its time limit. Check the folder or connection and try refreshing the library."}},
  windowsPort:{outputEndpointVolume:"Device volume in Windows: {{value}}%",outputEndpointMuted:"muted",outputEndpointReadOnly:"A reading at the time of the check. This check does not change volume.",errors:{windows_network_unavailable:"The network folder could not be verified. Check the connection and read access in Explorer, then select the folder again.",index_timeout:"The read could not finish in time. Check the media and try again."}},
  wizard: {
   step1Desc: "Choose Shared for the Windows default output, or Direct, under testing, for a specific device.",
   step1Shared: "Shared uses the Windows default audio output and allows audio from other applications.",
   step1Direct: "Direct requests exclusive WASAPI on the selected device. Exclusive opening and format are checked on playback.",
  },
  settings: {
   pipewireNotice: "The Windows default output may mix application audio and convert its format. Sonante uses software volume independently of the Windows master volume.",
   directNotice: "Direct requests exclusive WASAPI on the selected device, which another application may be using. Selecting this mode does not prove the physical output or bit-perfect playback.",
   dopDesc: "WASAPI Direct tries DoP for DSD when the endpoint accepts 24-bit exclusive PCM; if rejected, playback fails without PCM conversion.",
   dopOnlyDirect: "DSD DoP transport is available only in WASAPI Direct.",
   dopTransportNotice: "DoP carries DSD in marked PCM frames; it is not native DSD or a physical-signal certification.",
  },
  about: {
   audioEngineTitle: "Windows Audio Engine",
   audioEngineDesc: "MPD with WASAPI Shared, Direct under testing, and optional equalization through CamillaDSP.",
  },
 },
} as const;
