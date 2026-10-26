"""Worktrees page "How worktrees work" card copy.

Kept in its own module so the card's translations are easy to find and edit
without scrolling the much larger worktree glossary.
"""

GLOSSARY = {
    "zh-CN": {
        "How worktrees work": "工作树的用法",
        "A worktree is a second checkout of this repository on its own branch. It shares one Git history, so you can run several pi sessions side by side without stashing or switching branches.": "工作树是此仓库在独立分支上的第二份检出。它共享同一份 Git 历史，因此你可以并排运行多个 pi 会话，无需暂存或切换分支。",
        "Create one with Worktree: name the folder, pick a branch, and choose where it lives.": "用“工作树”创建：命名文件夹、选择分支，并决定它的位置。",
        "Open a row to work there. The Explorer, Files, terminal, Git, Review, and the agent all follow that folder.": "打开某一行即可在那里工作。资源管理器、文件、终端、Git、审查和智能体都会跟随该文件夹。",
        "Optional: add .orbit/worktree-setup.sh to run your own steps after creation, for example linking .env or sharing node_modules.": "可选：添加 .orbit/worktree-setup.sh，在创建后运行你自己的步骤，例如链接 .env 或共享 node_modules。",
        "Renaming moves the folder only; the Git branch always keeps its name.": "重命名只移动文件夹；Git 分支始终保留其名称。",
    },
    "ja": {
        "How worktrees work": "ワークツリーの使い方",
        "A worktree is a second checkout of this repository on its own branch. It shares one Git history, so you can run several pi sessions side by side without stashing or switching branches.": "ワークツリーは、このリポジトリを独自のブランチでチェックアウトした2つ目の作業ディレクトリです。Git 履歴を共有するため、スタッシュやブランチ切り替えなしで複数の pi セッションを並行して実行できます。",
        "Create one with Worktree: name the folder, pick a branch, and choose where it lives.": "「ワークツリー」で作成します。フォルダ名を付け、ブランチを選び、置き場所を決めます。",
        "Open a row to work there. The Explorer, Files, terminal, Git, Review, and the agent all follow that folder.": "行を開くとそこで作業できます。エクスプローラー、ファイル、ターミナル、Git、レビュー、エージェントはすべてそのフォルダに従います。",
        "Optional: add .orbit/worktree-setup.sh to run your own steps after creation, for example linking .env or sharing node_modules.": "任意：作成後に独自の処理を実行するには .orbit/worktree-setup.sh を追加します（例：.env のリンク、node_modules の共有）。",
        "Renaming moves the folder only; the Git branch always keeps its name.": "名前の変更はフォルダのみを移動します。Git ブランチ名は常に保持されます。",
    },
    "ko": {
        "How worktrees work": "워크트리 사용 방법",
        "A worktree is a second checkout of this repository on its own branch. It shares one Git history, so you can run several pi sessions side by side without stashing or switching branches.": "워크트리는 이 저장소를 자체 브랜치로 두 번째 체크아웃한 것입니다. 하나의 Git 기록을 공유하므로 스태시하거나 브랜치를 전환하지 않고도 여러 pi 세션을 나란히 실행할 수 있습니다.",
        "Create one with Worktree: name the folder, pick a branch, and choose where it lives.": "워크트리로 만드세요. 폴더 이름을 정하고, 브랜치를 고르고, 위치를 선택합니다.",
        "Open a row to work there. The Explorer, Files, terminal, Git, Review, and the agent all follow that folder.": "행을 열면 그곳에서 작업합니다. 탐색기, 파일, 터미널, Git, 리뷰, 에이전트가 모두 해당 폴더를 따릅니다.",
        "Optional: add .orbit/worktree-setup.sh to run your own steps after creation, for example linking .env or sharing node_modules.": "선택 사항: 생성 후 직접 단계를 실행하려면 .orbit/worktree-setup.sh를 추가하세요(예: .env 연결, node_modules 공유).",
        "Renaming moves the folder only; the Git branch always keeps its name.": "이름 변경은 폴더만 이동합니다. Git 브랜치는 항상 이름을 유지합니다.",
    },
    "es": {
        "How worktrees work": "Cómo funcionan los worktrees",
        "A worktree is a second checkout of this repository on its own branch. It shares one Git history, so you can run several pi sessions side by side without stashing or switching branches.": "Un worktree es una segunda copia de trabajo de este repositorio en su propia rama. Comparte un mismo historial de Git, así que puedes ejecutar varias sesiones de pi en paralelo sin guardar cambios ni cambiar de rama.",
        "Create one with Worktree: name the folder, pick a branch, and choose where it lives.": "Crea uno con Worktree: ponle nombre a la carpeta, elige una rama y dónde vivirá.",
        "Open a row to work there. The Explorer, Files, terminal, Git, Review, and the agent all follow that folder.": "Abre una fila para trabajar allí. El Explorador, Archivos, la terminal, Git, Revisión y el agente siguen esa carpeta.",
        "Optional: add .orbit/worktree-setup.sh to run your own steps after creation, for example linking .env or sharing node_modules.": "Opcional: añade .orbit/worktree-setup.sh para ejecutar tus propios pasos tras la creación, por ejemplo enlazar .env o compartir node_modules.",
        "Renaming moves the folder only; the Git branch always keeps its name.": "Renombrar solo mueve la carpeta; la rama de Git siempre conserva su nombre.",
    },
    "fr": {
        "How worktrees work": "Comment fonctionnent les worktrees",
        "A worktree is a second checkout of this repository on its own branch. It shares one Git history, so you can run several pi sessions side by side without stashing or switching branches.": "Un worktree est une seconde copie de travail de ce dépôt sur sa propre branche. Il partage un même historique Git : vous pouvez donc lancer plusieurs sessions pi en parallèle sans remiser ni changer de branche.",
        "Create one with Worktree: name the folder, pick a branch, and choose where it lives.": "Créez-en un avec Worktree : nommez le dossier, choisissez une branche et son emplacement.",
        "Open a row to work there. The Explorer, Files, terminal, Git, Review, and the agent all follow that folder.": "Ouvrez une ligne pour y travailler. L'explorateur, Fichiers, le terminal, Git, la revue et l'agent suivent tous ce dossier.",
        "Optional: add .orbit/worktree-setup.sh to run your own steps after creation, for example linking .env or sharing node_modules.": "Facultatif : ajoutez .orbit/worktree-setup.sh pour exécuter vos propres étapes après la création, par exemple lier .env ou partager node_modules.",
        "Renaming moves the folder only; the Git branch always keeps its name.": "Le renommage déplace uniquement le dossier ; la branche Git garde toujours son nom.",
    },
    "de": {
        "How worktrees work": "So funktionieren Worktrees",
        "A worktree is a second checkout of this repository on its own branch. It shares one Git history, so you can run several pi sessions side by side without stashing or switching branches.": "Ein Worktree ist eine zweite Auscheckung dieses Repositorys auf einem eigenen Branch. Er teilt sich eine Git-Historie, sodass du mehrere pi-Sitzungen parallel ausführen kannst, ohne zu stashen oder den Branch zu wechseln.",
        "Create one with Worktree: name the folder, pick a branch, and choose where it lives.": "Erstelle einen mit Worktree: Benenne den Ordner, wähle einen Branch und lege fest, wo er liegt.",
        "Open a row to work there. The Explorer, Files, terminal, Git, Review, and the agent all follow that folder.": "Öffne eine Zeile, um dort zu arbeiten. Explorer, Dateien, Terminal, Git, Review und der Agent folgen alle diesem Ordner.",
        "Optional: add .orbit/worktree-setup.sh to run your own steps after creation, for example linking .env or sharing node_modules.": "Optional: Füge .orbit/worktree-setup.sh hinzu, um nach dem Erstellen eigene Schritte auszuführen, zum Beispiel .env verlinken oder node_modules teilen.",
        "Renaming moves the folder only; the Git branch always keeps its name.": "Umbenennen verschiebt nur den Ordner; der Git-Branch behält immer seinen Namen.",
    },
    "pt-BR": {
        "How worktrees work": "Como funcionam os worktrees",
        "A worktree is a second checkout of this repository on its own branch. It shares one Git history, so you can run several pi sessions side by side without stashing or switching branches.": "Um worktree é uma segunda cópia de trabalho deste repositório em seu próprio branch. Ele compartilha um único histórico Git, então você pode executar várias sessões do pi em paralelo sem guardar alterações nem trocar de branch.",
        "Create one with Worktree: name the folder, pick a branch, and choose where it lives.": "Crie um com Worktree: nomeie a pasta, escolha um branch e onde ele ficará.",
        "Open a row to work there. The Explorer, Files, terminal, Git, Review, and the agent all follow that folder.": "Abra uma linha para trabalhar nela. O Explorador, Arquivos, o terminal, Git, Revisão e o agente seguem essa pasta.",
        "Optional: add .orbit/worktree-setup.sh to run your own steps after creation, for example linking .env or sharing node_modules.": "Opcional: adicione .orbit/worktree-setup.sh para executar suas próprias etapas após a criação, por exemplo vincular .env ou compartilhar node_modules.",
        "Renaming moves the folder only; the Git branch always keeps its name.": "Renomear move apenas a pasta; o branch do Git sempre mantém o nome.",
    },
    "ru": {
        "How worktrees work": "Как работают рабочие деревья",
        "A worktree is a second checkout of this repository on its own branch. It shares one Git history, so you can run several pi sessions side by side without stashing or switching branches.": "Рабочее дерево — это вторая рабочая копия этого репозитория на отдельной ветке. Оно разделяет одну историю Git, поэтому можно запускать несколько сеансов pi параллельно, не откладывая изменения и не переключая ветки.",
        "Create one with Worktree: name the folder, pick a branch, and choose where it lives.": "Создайте его кнопкой «Worktree»: задайте имя папки, выберите ветку и место.",
        "Open a row to work there. The Explorer, Files, terminal, Git, Review, and the agent all follow that folder.": "Откройте строку, чтобы работать там. Проводник, Файлы, терминал, Git, ревью и агент следуют за этой папкой.",
        "Optional: add .orbit/worktree-setup.sh to run your own steps after creation, for example linking .env or sharing node_modules.": "Необязательно: добавьте .orbit/worktree-setup.sh, чтобы выполнять собственные шаги после создания, например связать .env или общий node_modules.",
        "Renaming moves the folder only; the Git branch always keeps its name.": "Переименование перемещает только папку; ветка Git всегда сохраняет своё имя.",
    },
    "it": {
        "How worktrees work": "Come funzionano i worktree",
        "A worktree is a second checkout of this repository on its own branch. It shares one Git history, so you can run several pi sessions side by side without stashing or switching branches.": "Un worktree è una seconda copia di lavoro di questo repository sul proprio branch. Condivide un'unica cronologia Git, quindi puoi eseguire più sessioni pi in parallelo senza mettere da parte le modifiche né cambiare branch.",
        "Create one with Worktree: name the folder, pick a branch, and choose where it lives.": "Creane uno con Worktree: assegna un nome alla cartella, scegli un branch e dove risiederà.",
        "Open a row to work there. The Explorer, Files, terminal, Git, Review, and the agent all follow that folder.": "Apri una riga per lavorare lì. Esplora risorse, File, il terminale, Git, la revisione e l'agente seguono tutti quella cartella.",
        "Optional: add .orbit/worktree-setup.sh to run your own steps after creation, for example linking .env or sharing node_modules.": "Facoltativo: aggiungi .orbit/worktree-setup.sh per eseguire i tuoi passaggi dopo la creazione, ad esempio collegare .env o condividere node_modules.",
        "Renaming moves the folder only; the Git branch always keeps its name.": "La rinomina sposta solo la cartella; il branch Git conserva sempre il suo nome.",
    },
}
