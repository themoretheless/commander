# 99 репозиториев для изучения при развитии Commander

Проверено 27 октября 2026 года. Strata и сам Commander не входят в эти 99.

**Это отбор по полезности для Commander, а не мировой рейтинг по звёздам.** У каждого проекта проверены GitHub API и README. Все 99 доступны, не архивированы, не являются GitHub-форками и имеют не менее 100 звёзд на момент проверки. У всех `pushed_at` находится в пределах последних двух лет; это сигнал обновления репозитория, а не доказательство качества сопровождения или даты последнего релиза.

Внутри: **65 файловых менеджеров, каталогизаторов и файловых компонентов**, **9 браузеров файлов для редакторов**, **25 смежных инструментов** поиска, анализа места, синхронизации и архивирования. Смежные инструменты включены как источники конкретных функций; они не представлены как полноценные аналоги Strata.

Столбец «Что изучить» — моя рекомендация на основе документации проекта. Это не результат аудита реализации, не сравнение скорости и не подтверждение, что соответствующая возможность отсутствует в Commander. Проекты не устанавливались и не запускались; код в Commander в рамках этого поиска не переносился.

У официальных зеркал GNOME/KDE/Xfce число звёзд GitHub существенно хуже отражает известность, чем у проектов, для которых GitHub — основной хостинг. Такие зеркала помечены знаком †. Язык приведён буквально из GitHub API; например, Fluent у Czkawka отражает статистику файлов переводов, а не отсутствие Rust-кода.

Cmdr отдельно помечен **source-available / BSL-1.1**. Для остальных проектов автоматически определённая лицензия и её источник сохранены в JSON; `NOASSERTION` или отсутствие поля не трактуются как отсутствие лицензии. Этот каталог не является разрешением копировать код.

## С чего начать

| Приоритет | Репозиторий | ★ | Почему полезен для Commander |
| --- | --- | ---: | --- |
| 1 | [houqp/kiorg](https://github.com/houqp/kiorg) | 463 | Rust + egui; превью, плагины, встроенный терминал и undo/redo — ближайший технический аналог. [README](https://github.com/houqp/kiorg/blob/main/README.md) |
| 2 | [mikekazakov/nimble-commander](https://github.com/mikekazakov/nimble-commander) | 752 | Нативный двухпанельный macOS UX и клавиатурная навигация. [README](https://github.com/mikekazakov/nimble-commander/blob/main/README.md) |
| 3 | [sxyazi/yazi](https://github.com/sxyazi/yazi) | 43 047 | Асинхронный файловый UI на Rust, задачи, превью, VFS и Lua-плагины. [README](https://github.com/sxyazi/yazi/blob/main/README.md) |
| 4 | [doublecmd/doublecmd](https://github.com/doublecmd/doublecmd) | 4 688 | Зрелая двухпанельная модель и большой набор повседневных операций. [README](https://github.com/doublecmd/doublecmd/blob/master/README.md) |
| 5 | [KDE/krusader](https://github.com/KDE/krusader) | 183 | Двухпанельная commander-модель для Linux. [README](https://github.com/KDE/krusader/blob/master/README.md) |
| 6 | [elementary/files](https://github.com/elementary/files) | 366 | Режимы значков, списка и колонок — особенно полезен для Miller columns. [README](https://github.com/elementary/files/blob/main/README.md) |
| 7 | [files-community/Files](https://github.com/files-community/Files) | 46 236 | Современный интерфейс, многозадачность и теги файлов в Windows. [README](https://github.com/files-community/Files/blob/main/.github/README.md) |
| 8 | [spacedriveapp/spacedrive](https://github.com/spacedriveapp/spacedrive) | 39 313 | Индекс файлов, постоянная идентичность и единый поиск по разным хранилищам. [README](https://github.com/spacedriveapp/spacedrive/blob/main/README.md) |
| 9 | [pop-os/cosmic-files](https://github.com/pop-os/cosmic-files) | 309 | Графический файловый менеджер на Rust и архитектура COSMIC-приложения. [README](https://github.com/pop-os/cosmic-files/blob/master/README.md) |
| 10 | [rclone/rclone](https://github.com/rclone/rclone) | 60 567 | Адаптеры облачных хранилищ и перенос между провайдерами. [README](https://github.com/rclone/rclone/blob/master/README.md) |
| 11 | [mickael-kerjean/filestash](https://github.com/mickael-kerjean/filestash) | 14 831 | Единый UI для SFTP, SMB, S3, WebDAV и других провайдеров. [README](https://github.com/mickael-kerjean/filestash/blob/master/README.md) |
| 12 | [vdavid/cmdr](https://github.com/vdavid/cmdr) | 119 | Rust + Tauri, macOS, две панели; BSL-1.1 source-available, не обычная OSS-лицензия. [README](https://github.com/vdavid/cmdr/blob/main/README.md) |

Kiorg особенно близок текущему стеку Commander: Rust + egui. Nimble Commander и Cmdr полезны как macOS-ориентиры; elementary Files — для режима колонок; Yazi — для асинхронного UI, задач и превью; Filestash и rclone — для сетевых провайдеров.

## Все 99 репозиториев

### Настольные файловые менеджеры и каталогизаторы — 26

| № | Репозиторий | ★ | Язык GitHub | Что изучить |
| ---: | --- | ---: | --- | --- |
| 1 | [houqp/kiorg](https://github.com/houqp/kiorg) | 463 | Rust | Rust + egui; превью, плагины, встроенный терминал и undo/redo — ближайший технический аналог. [README](https://github.com/houqp/kiorg/blob/main/README.md) |
| 2 | [mikekazakov/nimble-commander](https://github.com/mikekazakov/nimble-commander) | 752 | C++ | Нативный двухпанельный macOS UX и клавиатурная навигация. [README](https://github.com/mikekazakov/nimble-commander/blob/main/README.md) |
| 3 | [doublecmd/doublecmd](https://github.com/doublecmd/doublecmd) | 4 688 | Pascal | Зрелая двухпанельная модель и большой набор повседневных операций. [README](https://github.com/doublecmd/doublecmd/blob/master/README.md) |
| 4 | [KDE/krusader](https://github.com/KDE/krusader) † | 183 | C++ | Двухпанельная commander-модель для Linux. [README](https://github.com/KDE/krusader/blob/master/README.md) |
| 5 | [elementary/files](https://github.com/elementary/files) | 366 | Vala | Режимы значков, списка и колонок — особенно полезен для Miller columns. [README](https://github.com/elementary/files/blob/main/README.md) |
| 6 | [files-community/Files](https://github.com/files-community/Files) | 46 236 | C# | Современный интерфейс, многозадачность и теги файлов в Windows. [README](https://github.com/files-community/Files/blob/main/.github/README.md) |
| 7 | [spacedriveapp/spacedrive](https://github.com/spacedriveapp/spacedrive) | 39 313 | Rust | Индекс файлов, постоянная идентичность и единый поиск по разным хранилищам. [README](https://github.com/spacedriveapp/spacedrive/blob/main/README.md) |
| 8 | [pop-os/cosmic-files](https://github.com/pop-os/cosmic-files) | 309 | Rust | Графический файловый менеджер на Rust и архитектура COSMIC-приложения. [README](https://github.com/pop-os/cosmic-files/blob/master/README.md) |
| 9 | [vdavid/cmdr](https://github.com/vdavid/cmdr) | 119 | Rust | Rust + Tauri, macOS, две панели; BSL-1.1 source-available, не обычная OSS-лицензия. [README](https://github.com/vdavid/cmdr/blob/main/README.md) |
| 10 | [TagStudioDev/TagStudio](https://github.com/TagStudioDev/TagStudio) | 7 255 | Python | Организация медиатеки и файлов по пользовательским метаданным. [README](https://github.com/TagStudioDev/TagStudio/blob/main/README.md) |
| 11 | [aleksey-hoffman/sigma-file-manager](https://github.com/aleksey-hoffman/sigma-file-manager) | 6 734 | TypeScript | Современные сценарии организации и просмотра файлов. [README](https://github.com/aleksey-hoffman/sigma-file-manager/blob/main/README.md) |
| 12 | [kimlimjustin/xplorer](https://github.com/kimlimjustin/xplorer) | 5 695 | TypeScript | Современный Rust/React-файловый менеджер с расширяемым интерфейсом. [README](https://github.com/kimlimjustin/xplorer/blob/next/README.md) |
| 13 | [tagspaces/tagspaces](https://github.com/tagspaces/tagspaces) | 5 348 | TypeScript | Офлайн-каталогизация документов и навигация по тегам. [README](https://github.com/tagspaces/tagspaces/blob/develop/README.md) |
| 14 | [derceg/explorerplusplus](https://github.com/derceg/explorerplusplus) | 3 325 | C++ | Лёгкий Windows Explorer-подобный интерфейс с вкладками. [README](https://github.com/derceg/explorerplusplus/blob/master/README.md) |
| 15 | [linuxmint/nemo](https://github.com/linuxmint/nemo) | 1 575 | C | Файловые сценарии Cinnamon и расширение настольного менеджера. [README](https://github.com/linuxmint/nemo/blob/master/README.md) |
| 16 | [mucommander/mucommander](https://github.com/mucommander/mucommander) | 1 292 | Java | Кроссплатформенная двухпанельная работа с файлами. [README](https://github.com/mucommander/mucommander/blob/master/README.md) |
| 17 | [KDE/dolphin](https://github.com/KDE/dolphin) † | 776 | C++ | Настольная навигация, работа с носителями и зрелая организация интерфейса. [README](https://github.com/KDE/dolphin/blob/master/README.md) |
| 18 | [lxqt/pcmanfm-qt](https://github.com/lxqt/pcmanfm-qt) | 575 | C++ | Лёгкий Qt-интерфейс и разделение представления и файлового слоя. [README](https://github.com/lxqt/pcmanfm-qt/blob/master/README.md) |
| 19 | [IngvarX/Camelot](https://github.com/IngvarX/Camelot) | 548 | C# | Лёгкая двухпанельная компоновка для нескольких ОС. [README](https://github.com/IngvarX/Camelot/blob/master/README.md) |
| 20 | [VioletGiraffe/file-commander](https://github.com/VioletGiraffe/file-commander) | 456 | C++ | Кроссплатформенные две панели и плагины. [README](https://github.com/VioletGiraffe/file-commander/blob/master/README.md) |
| 21 | [MeanEYE/Sunflower](https://github.com/MeanEYE/Sunflower) | 448 | Python | Две панели и расширение возможностей плагинами. [README](https://github.com/MeanEYE/Sunflower/blob/develop/README.md) |
| 22 | [GNOME/nautilus](https://github.com/GNOME/nautilus) † | 423 | C | Повседневные файловые сценарии и интеграция с Linux desktop. [README](https://github.com/GNOME/nautilus/blob/main/README.md) |
| 23 | [mate-desktop/caja](https://github.com/mate-desktop/caja) | 318 | C | Файловая навигация и интеграция с окружением MATE. [README](https://github.com/mate-desktop/caja/blob/master/README.md) |
| 24 | [xfce-mirror/thunar](https://github.com/xfce-mirror/thunar) † | 280 | C | Лёгкий файловый менеджер и организация компактного desktop UX. [README](https://github.com/xfce-mirror/thunar/blob/master/README.md) |
| 25 | [lxde/pcmanfm](https://github.com/lxde/pcmanfm) | 255 | C | Минималистичный настольный файловый менеджер. [README](https://github.com/lxde/pcmanfm/blob/master/README) |
| 26 | [rodlie/qtfm](https://github.com/rodlie/qtfm) | 182 | C++ | Пользовательские команды, настраиваемые клавиши и drag-and-drop. [README](https://github.com/rodlie/qtfm/blob/main/README.md) |

### Терминальные файловые менеджеры и навигаторы — 19

| № | Репозиторий | ★ | Язык GitHub | Что изучить |
| ---: | --- | ---: | --- | --- |
| 27 | [sxyazi/yazi](https://github.com/sxyazi/yazi) | 43 047 | Rust | Асинхронный файловый UI на Rust, задачи, превью, VFS и Lua-плагины. [README](https://github.com/sxyazi/yazi/blob/main/README.md) |
| 28 | [yorukot/superfile](https://github.com/yorukot/superfile) | 23 944 | Go | Современная терминальная компоновка и файловые превью. [README](https://github.com/yorukot/superfile/blob/main/README.md) |
| 29 | [jarun/nnn](https://github.com/jarun/nnn) | 22 097 | C | Минималистичная клавиатурная работа с файлами и экосистема расширений. [README](https://github.com/jarun/nnn/blob/master/README.md) |
| 30 | [ranger/ranger](https://github.com/ranger/ranger) | 17 440 | Python | Навигация по колонкам и превью выбранного файла. [README](https://github.com/ranger/ranger/blob/master/README.md) |
| 31 | [Canop/broot](https://github.com/Canop/broot) | 13 104 | Rust | Интерактивное дерево каталогов и быстрый поиск в нём. [README](https://github.com/Canop/broot/blob/main/README.md) |
| 32 | [gokcehan/lf](https://github.com/gokcehan/lf) | 9 577 | Go | Компактный клавиатурный менеджер и расширение внешними командами. [README](https://github.com/gokcehan/lf/blob/master/README.md) |
| 33 | [sayanarijit/xplr](https://github.com/sayanarijit/xplr) | 4 846 | Rust | Настраиваемый файловый UI и скриптовое расширение сценариев. [README](https://github.com/sayanarijit/xplr/blob/main/README.md) |
| 34 | [kamiyaa/joshuto](https://github.com/kamiyaa/joshuto) | 3 739 | Rust | Ranger-подобная навигация на Rust. [README](https://github.com/kamiyaa/joshuto/blob/main/README.md) |
| 35 | [antonmedv/walk](https://github.com/antonmedv/walk) | 3 649 | Go | Быстрая навигация по каталогам как замена cd/ls. [README](https://github.com/antonmedv/walk/blob/master/README.md) |
| 36 | [vifm/vifm](https://github.com/vifm/vifm) | 3 280 | C | Двухпанельные сценарии и Vim-подобные команды. [README](https://github.com/vifm/vifm/blob/master/README.md) |
| 37 | [FarGroup/FarManager](https://github.com/FarGroup/FarManager) | 2 250 | C++ | Зрелые клавиатурные сценарии и работа с файлами и архивами. [README](https://github.com/FarGroup/FarManager/blob/master/README.md) |
| 38 | [elfmz/far2l](https://github.com/elfmz/far2l) | 2 229 | C++ | Перенос модели FAR на Linux и интеграция с терминалом. [README](https://github.com/elfmz/far2l/blob/master/README.md) |
| 39 | [leo-arch/clifm](https://github.com/leo-arch/clifm) | 1 735 | C | Связь файлового менеджера с командной строкой. [README](https://github.com/leo-arch/clifm/blob/master/README.md) |
| 40 | [MidnightCommander/mc](https://github.com/MidnightCommander/mc) | 1 028 | C | Классическая commander-модель и проверенные файловые сценарии. [README](https://github.com/MidnightCommander/mc/blob/master/README) |
| 41 | [kyoheiu/felix](https://github.com/kyoheiu/felix) | 923 | Rust | Файловые операции и Vim-подобная навигация на Rust. [README](https://github.com/kyoheiu/felix/blob/develop/README.md) |
| 42 | [GiorgosXou/TUIFIManager](https://github.com/GiorgosXou/TUIFIManager) | 828 | Python | Визуальный терминальный файловый интерфейс. [README](https://github.com/GiorgosXou/TUIFIManager/blob/master/README.md) |
| 43 | [juftin/browsr](https://github.com/juftin/browsr) | 648 | Python | Единая навигация по разным файловым системам. [README](https://github.com/juftin/browsr/blob/main/README.md) |
| 44 | [mistakenelf/fm](https://github.com/mistakenelf/fm) | 637 | Go | Компактный терминальный UX управления файлами. [README](https://github.com/mistakenelf/fm/blob/main/README.md) |
| 45 | [NSPC911/rovr](https://github.com/NSPC911/rovr) | 415 | Python | Современный терминальный интерфейс и организация файловых действий. [README](https://github.com/NSPC911/rovr/blob/master/README.md) |

### Веб-менеджеры, удалённые файлы и файловые компоненты — 16

| № | Репозиторий | ★ | Язык GitHub | Что изучить |
| ---: | --- | ---: | --- | --- |
| 46 | [mickael-kerjean/filestash](https://github.com/mickael-kerjean/filestash) | 14 831 | Go | Единый UI для SFTP, SMB, S3, WebDAV и других провайдеров. [README](https://github.com/mickael-kerjean/filestash/blob/master/README.md) |
| 47 | [nextcloud/server](https://github.com/nextcloud/server) | 37 311 | PHP | Файлы, общий доступ и серверные сценарии хранения. [README](https://github.com/nextcloud/server/blob/master/README.md) |
| 48 | [cloudreve/cloudreve](https://github.com/cloudreve/cloudreve) | 28 891 | Go | Единая файловая оболочка над несколькими хранилищами. [README](https://github.com/cloudreve/cloudreve/blob/master/README.md) |
| 49 | [drakkan/sftpgo](https://github.com/drakkan/sftpgo) | 12 714 | Go | SFTP/FTP/WebDAV и разные backend-хранилища. [README](https://github.com/drakkan/sftpgo/blob/main/README.md) |
| 50 | [gtsteffaniak/filebrowser](https://github.com/gtsteffaniak/filebrowser) | 8 921 | Go | Актуальный самостоятельный File Browser Quantum для веб-доступа к файлам. [README](https://github.com/gtsteffaniak/filebrowser/blob/main/README.md) |
| 51 | [owncloud/core](https://github.com/owncloud/core) | 8 840 | PHP | Файловый веб-интерфейс и DAV-интеграция. [README](https://github.com/owncloud/core/blob/master/README.md) |
| 52 | [prasathmani/tinyfilemanager](https://github.com/prasathmani/tinyfilemanager) | 6 001 | PHP | Минимальный браузерный интерфейс файловых операций. [README](https://github.com/prasathmani/tinyfilemanager/blob/master/README.md) |
| 53 | [Studio-42/elFinder](https://github.com/Studio-42/elFinder) | 4 813 | JavaScript | Встраиваемый веб-менеджер с файловым интерфейсом. [README](https://github.com/Studio-42/elFinder/blob/master/README.md) |
| 54 | [kalcaddle/kodbox](https://github.com/kalcaddle/kodbox) | 3 366 | PHP | Веб-организация файлов и встроенное редактирование. [README](https://github.com/kalcaddle/kodbox/blob/main/README.md) |
| 55 | [filegator/filegator](https://github.com/filegator/filegator) | 3 082 | PHP | Многопользовательский файловый менеджер. [README](https://github.com/filegator/filegator/blob/master/README.md) |
| 56 | [owncloud/ocis](https://github.com/owncloud/ocis) | 2 169 | Go | Архитектура файлового сервиса и масштабируемого хранения. [README](https://github.com/owncloud/ocis/blob/master/README.md) |
| 57 | [coderaiser/cloudcmd](https://github.com/coderaiser/cloudcmd) | 2 036 | JavaScript | Файловый менеджер с консолью и редактором в браузере. [README](https://github.com/coderaiser/cloudcmd/blob/master/README.md) |
| 58 | [error311/FileRise](https://github.com/error311/FileRise) | 1 022 | JavaScript | Возобновляемые загрузки, WebDAV и права доступа к файлам. [README](https://github.com/error311/FileRise/blob/master/README.md) |
| 59 | [linagora/twake-drive](https://github.com/linagora/twake-drive) | 990 | JavaScript | Командная работа с файлами через веб-интерфейс. [README](https://github.com/linagora/twake-drive/blob/master/README.md) |
| 60 | [n1crack/vuefinder](https://github.com/n1crack/vuefinder) | 607 | Vue | Встраиваемый файловый UI на Vue. [README](https://github.com/n1crack/vuefinder/blob/master/README.md) |
| 61 | [zer0tonin/Mikochi](https://github.com/zer0tonin/Mikochi) | 377 | JavaScript | Простой браузер файлов NAS/сервера с потоковым просмотром. [README](https://github.com/zer0tonin/Mikochi/blob/main/README.md) |

### Файловые менеджеры Android — 4

| № | Репозиторий | ★ | Язык GitHub | Что изучить |
| ---: | --- | ---: | --- | --- |
| 62 | [zhanghai/MaterialFiles](https://github.com/zhanghai/MaterialFiles) | 9 323 | Kotlin | Material-интерфейс файлового менеджера Android. [README](https://github.com/zhanghai/MaterialFiles/blob/master/README.md) |
| 63 | [TeamAmaze/AmazeFileManager](https://github.com/TeamAmaze/AmazeFileManager) | 6 445 | Kotlin | Полноценный файловый менеджер с Material UI. [README](https://github.com/TeamAmaze/AmazeFileManager/blob/release/4.0/README.md) |
| 64 | [1hakr/AnExplorer](https://github.com/1hakr/AnExplorer) | 2 048 | Java | Адаптация файлового UI к телефону, планшету, TV и другим устройствам. [README](https://github.com/1hakr/AnExplorer/blob/master/README.md) |
| 65 | [FossifyOrg/File-Manager](https://github.com/FossifyOrg/File-Manager) | 1 836 | Kotlin | Простой файловый UX без рекламных сценариев. [README](https://github.com/FossifyOrg/File-Manager/blob/main/README.md) |

### Файловые браузеры редакторов — 9

| № | Репозиторий | ★ | Язык GitHub | Что изучить |
| ---: | --- | ---: | --- | --- |
| 66 | [preservim/nerdtree](https://github.com/preservim/nerdtree) | 20 085 | Vim Script | Зрелые сценарии дерева файлов и клавиатурной навигации. [README](https://github.com/preservim/nerdtree/blob/master/README.markdown) |
| 67 | [nvim-tree/nvim-tree.lua](https://github.com/nvim-tree/nvim-tree.lua) | 8 669 | Lua | Настраиваемое дерево файлов для редактора. [README](https://github.com/nvim-tree/nvim-tree.lua/blob/master/README.md) |
| 68 | [stevearc/oil.nvim](https://github.com/stevearc/oil.nvim) | 6 965 | Lua | Редактирование файловой структуры как текстового буфера. [README](https://github.com/stevearc/oil.nvim/blob/master/README.md) |
| 69 | [nvim-neo-tree/neo-tree.nvim](https://github.com/nvim-neo-tree/neo-tree.nvim) | 5 634 | Lua | Разные представления дерева и контекст файлового проекта. [README](https://github.com/nvim-neo-tree/neo-tree.nvim/blob/main/README.md) |
| 70 | [Alexander-Miller/treemacs](https://github.com/Alexander-Miller/treemacs) | 2 367 | Emacs Lisp | Проектное дерево файлов и организация рабочих пространств. [README](https://github.com/Alexander-Miller/treemacs/blob/master/README.org) |
| 71 | [nvim-telescope/telescope-file-browser.nvim](https://github.com/nvim-telescope/telescope-file-browser.nvim) | 1 945 | Lua | Поиск и файловые действия в одном интерфейсе. [README](https://github.com/nvim-telescope/telescope-file-browser.nvim/blob/master/README.md) |
| 72 | [ms-jpq/chadtree](https://github.com/ms-jpq/chadtree) | 1 689 | Python | Файловый менеджер внутри редактора. [README](https://github.com/ms-jpq/chadtree/blob/chad/README.md) |
| 73 | [justinmk/vim-dirvish](https://github.com/justinmk/vim-dirvish) | 1 280 | Vim Script | Минималистичный просмотр каталогов средствами редактора. [README](https://github.com/justinmk/vim-dirvish/blob/master/README.md) |
| 74 | [FylerOrg/fyler.nvim](https://github.com/FylerOrg/fyler.nvim) | 771 | Lua | Редактируемое дерево файлов и применение изменений из буфера. [README](https://github.com/FylerOrg/fyler.nvim/blob/main/README.md) |

### Поиск, переходы и представление каталогов — 6

| № | Репозиторий | ★ | Язык GitHub | Что изучить |
| ---: | --- | ---: | --- | --- |
| 75 | [junegunn/fzf](https://github.com/junegunn/fzf) | 83 815 | Go | Нечёткий поиск, выбор элементов и интерактивные списки. [README](https://github.com/junegunn/fzf/blob/master/README.md) |
| 76 | [BurntSushi/ripgrep](https://github.com/BurntSushi/ripgrep) | 69 485 | Rust | Рекурсивный поиск содержимого с учётом ignore-файлов. [README](https://github.com/BurntSushi/ripgrep/blob/master/README.md) |
| 77 | [sharkdp/fd](https://github.com/sharkdp/fd) | 44 867 | Rust | Поиск файлов по имени и удобная фильтрация обхода. [README](https://github.com/sharkdp/fd/blob/master/README.md) |
| 78 | [ajeetdsouza/zoxide](https://github.com/ajeetdsouza/zoxide) | 40 315 | Rust | Ранжирование часто посещаемых каталогов. [README](https://github.com/ajeetdsouza/zoxide/blob/main/README.md) |
| 79 | [eza-community/eza](https://github.com/eza-community/eza) | 23 671 | Rust | Представление атрибутов файлов и читаемые списки. [README](https://github.com/eza-community/eza/blob/main/README.md) |
| 80 | [lsd-rs/lsd](https://github.com/lsd-rs/lsd) | 16 278 | Rust | Компактные списки, значки и оформление файловых атрибутов. [README](https://github.com/lsd-rs/lsd/blob/main/README.md) |

### Анализ места и поиск дубликатов — 6

| № | Репозиторий | ★ | Язык GitHub | Что изучить |
| ---: | --- | ---: | --- | --- |
| 81 | [qarmin/czkawka](https://github.com/qarmin/czkawka) | 34 564 | Fluent | Поиск дубликатов, пустых каталогов и похожих изображений. [README](https://github.com/qarmin/czkawka/blob/master/README.md) |
| 82 | [bootandy/dust](https://github.com/bootandy/dust) | 12 550 | Rust | Наглядное представление размеров каталогов. [README](https://github.com/bootandy/dust/blob/master/README.md) |
| 83 | [Byron/dua-cli](https://github.com/Byron/dua-cli) | 6 392 | Rust | Быстрый анализ занятого места и интерактивная очистка. [README](https://github.com/Byron/dua-cli/blob/main/README.md) |
| 84 | [dundee/gdu](https://github.com/dundee/gdu) | 6 156 | Go | Интерактивный анализ размеров каталогов. [README](https://github.com/dundee/gdu/blob/master/README.md) |
| 85 | [pkolaczk/fclones](https://github.com/pkolaczk/fclones) | 2 980 | Rust | Поэтапное сравнение дубликатов, dry-run и машиночитаемые отчёты. [README](https://github.com/pkolaczk/fclones/blob/main/README.md) |
| 86 | [sahib/rmlint](https://github.com/sahib/rmlint) | 2 447 | C | Дубликаты, битые ссылки и другие проблемы файлового дерева. [README](https://github.com/sahib/rmlint/blob/master/README.rst) |

### Передача, синхронизация и резервные копии — 8

| № | Репозиторий | ★ | Язык GitHub | Что изучить |
| ---: | --- | ---: | --- | --- |
| 87 | [rclone/rclone](https://github.com/rclone/rclone) | 60 567 | Go | Адаптеры облачных хранилищ и перенос между провайдерами. [README](https://github.com/rclone/rclone/blob/master/README.md) |
| 88 | [localsend/localsend](https://github.com/localsend/localsend) | 95 979 | Dart | Локальная отправка файлов между устройствами. [README](https://github.com/localsend/localsend/blob/main/README.md) |
| 89 | [syncthing/syncthing](https://github.com/syncthing/syncthing) | 89 793 | Go | Непрерывная синхронизация и разрешение конфликтов между устройствами. [README](https://github.com/syncthing/syncthing/blob/main/README.md) |
| 90 | [schollz/croc](https://github.com/schollz/croc) | 40 774 | Go | Передача файлов между компьютерами. [README](https://github.com/schollz/croc/blob/main/README.md) |
| 91 | [restic/restic](https://github.com/restic/restic) | 36 829 | Go | Проверяемые резервные копии и восстановление данных. [README](https://github.com/restic/restic/blob/master/README.md) |
| 92 | [borgbackup/borg](https://github.com/borgbackup/borg) | 13 884 | Python | Дедупликация, архивирование и шифрование резервных копий. [README](https://github.com/borgbackup/borg/blob/master/README.rst) |
| 93 | [abraunegg/onedrive](https://github.com/abraunegg/onedrive) | 12 943 | D | Синхронизация с OneDrive и обработка облачного состояния. [README](https://github.com/abraunegg/onedrive/blob/master/readme.md) |
| 94 | [RsyncProject/rsync](https://github.com/RsyncProject/rsync) | 5 316 | C | Инкрементальный перенос и delta-алгоритм. [README](https://github.com/RsyncProject/rsync/blob/master/README.md) |

### Архивы и архивные файловые системы — 5

| № | Репозиторий | ★ | Язык GitHub | Что изучить |
| ---: | --- | ---: | --- | --- |
| 95 | [M2Team/NanaZip](https://github.com/M2Team/NanaZip) | 16 008 | C++ | Интеграция архиватора в современный Windows UX. [README](https://github.com/M2Team/NanaZip/blob/main/ReadMe.md) |
| 96 | [peazip/PeaZip](https://github.com/peazip/PeaZip) | 8 024 | Pascal | Графическая работа с архивами и большим набором форматов. [README](https://github.com/peazip/PeaZip/blob/sources/README.md) |
| 97 | [ouch-org/ouch](https://github.com/ouch-org/ouch) | 3 804 | Rust | Удобная упаковка и распаковка из командной строки. [README](https://github.com/ouch-org/ouch/blob/main/README.md) |
| 98 | [mholt/archives](https://github.com/mholt/archives) | 447 | Go | Общий интерфейс архивов, сжатия и виртуальных файловых систем. [README](https://github.com/mholt/archives/blob/main/README.md) |
| 99 | [KDE/ark](https://github.com/KDE/ark) † | 144 | C++ | Архивы как часть настольного файлового сценария. [README](https://github.com/KDE/ark/blob/master/README.md) |

## Проверка и границы отбора

- Ровно 99 разных GitHub repository ID; повторов и переадресованных дублей нет.
- 99/99: доступны метаданные; 99/99: прочитан README; 0 архивных, отключённых и GitHub-форков.
- Звёзды, язык, даты push, обнаруженная лицензия, ссылки API/README и SHA-256 полученного текста README сохранены в [снимке данных](99-file-projects-2026-10-27.json).
- Не включён архивный `filebrowser/filebrowser`; включён самостоятельный `gtsteffaniak/filebrowser` (Quantum).
- Не включён `zhuxb711/RX-Explorer`: README помечает UWP-версию устаревшей. Не включён `kalcaddle/KodExplorer`: README рекомендует kodbox, который уже есть в подборке.
- Отсечены найденные, но давно не обновлявшиеся NewBreeze, cfiles, goful и diskonaut. Недоступные URL не считаются репозиториями и не заменяются догадками.
- Более низкие звёзды Kiorg/COSMIC Files/Nimble Commander/Cmdr оправданы непосредственным сходством со стеком или macOS-сценариями Commander; популярность не подменяет релевантность.
- Полнота глобальной выборки GitHub и универсальное превосходство этих 99 проектов не заявляются.
