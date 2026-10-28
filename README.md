# Roblox Studio nativo para Linux

Projeto para portar o Roblox Studio a uma aplicação nativa de Linux. A meta de longo prazo é cobrir os fluxos do Studio de forma independente, com comportamento e compatibilidade verificados; executar o programa Windows por uma camada de compatibilidade não faz parte do objetivo.

## Estado

O projeto está na etapa de planejamento e pesquisa. Ainda não há código do aplicativo, build ou release. O [roadmap](ROADMAP.md) define fases, critérios de saída, métricas de qualidade e desempenho e decisões técnicas pendentes.

## Princípios

- Usar o Studio original no Windows como referência de comportamento, não como teto de desempenho.
- Guiar a análise com REA, evidências rastreáveis e testes diferenciais.
- Evitar perda silenciosa de dados e otimizações sem diagnóstico e medição antes/depois.
- Manter a implementação e o runtime nativos em Linux.

## Documentação do projeto

- [ROADMAP.md](ROADMAP.md): fases, escopo, critérios de aceitação, métricas e fontes técnicas.
- [AGENTS.md](AGENTS.md): instruções de execução técnica, pesquisa, qualidade, otimização e colaboração para agentes de código.
