# リリース・ストア提出

## 現状

この文書と `store/ja-JP/` の文面は提出素材です。本番用の署名・アップロード・公開は未実施です。アプリIDは既存値 `dev.kunianki.app` を維持します。ストアアカウント所有者が利用権限と最終IDを確認・承認するまで、ID登録やストア操作は行いません。

## ローカルのリリースビルド

### Android App Bundle

リポジトリのルートで実行します。現在のAndroid設定は `compileSdk 36`、`targetSdk 36`、`versionCode 1`、`versionName 0.1.0` です。公開のたびに `versionCode` を増加させ、`versionName` を公開版に合わせてください。

```sh
./android/gradlew -p android :app:bundleRelease
```

成果物は `android/app/build/outputs/bundle/release/app-release.aab` です。署名設定は次の4つの環境変数をすべて設定した場合に限り、指定されたアップロード鍵で署名します。

- `KUNIANKI_KEYSTORE_PATH`
- `KUNIANKI_KEYSTORE_PASSWORD`
- `KUNIANKI_KEY_ALIAS`
- `KUNIANKI_KEY_PASSWORD`

4つとも未設定なら生成されるAABは未署名で、Google Playにはアップロードできません。一部だけ設定した場合は設定エラーになります。鍵とパスワードはリポジトリに保存せず、安全な環境から渡し、ログやチャットに出力しないでください。本番鍵は生成・登録していません。

Google Play App Signingでは、開発者のアップロード鍵はアップロード元を確認する鍵です。Play Consoleで選択または生成するアプリ署名鍵は、Google Playが利用者へ配布するAPKへの署名に使われます。アカウント所有者がPlay App Signingへの登録とアップロード鍵の保管・管理者を決め、鍵の管理を確立するまでは署名済みAABを作らないでください。

### iOS App Storeアーカイブ

Xcode 26以降とiOS 26 SDK、XcodeGen、およびアカウント所有者が認可したApple Developerチームが必要です。リポジトリのルートで実行します。

```sh
(cd ios && xcodegen generate)
: "${KUNIANKI_APPLE_TEAM_ID:?アカウント所有者が指定したApple Developer Team IDが必要です}"
mkdir -p target/release/ios
xcodebuild \
  -project ios/kunianki.xcodeproj \
  -scheme kunianki \
  -configuration Release \
  -destination 'generic/platform=iOS' \
  -archivePath "$PWD/target/release/ios/kunianki.xcarchive" \
  -allowProvisioningUpdates \
  DEVELOPMENT_TEAM="$KUNIANKI_APPLE_TEAM_ID" \
  CODE_SIGN_STYLE=Automatic \
  archive
xcodebuild -exportArchive \
  -archivePath target/release/ios/kunianki.xcarchive \
  -exportOptionsPlist ios/ExportOptions.plist \
  -exportPath target/release/ios/export \
  -allowProvisioningUpdates
```

成果物は `target/release/ios/kunianki.xcarchive` と `target/release/ios/export/` 内の `.ipa` です。`ios/ExportOptions.plist` は `app-store-connect` 配布、`automatic` 署名を指定します。公開版・ビルド番号は `ios/project.yml` で更新し、XcodeGenで再生成してください。`CODE_SIGNING_ALLOWED=NO` は未署名の検証専用であり、この配布アーカイブには使いません。

ローカルで生成した未署名の検証用アーカイブは `target/release/ios/kunianki-unsigned.xcarchive` です。arm64、iPhone/iPad、最低iOS 16.0、AppIcon・起動画像・プライバシーマニフェストを確認済みで、本番用の署名やIPA書き出しは行っていません。

## ストア掲載情報・提出物

日本語の入力原稿は [`google-play.json`](../store/ja-JP/google-play.json) と [`app-store.json`](../store/ja-JP/app-store.json) にあります。アプリ名はストア名・ネイティブ表示名の「国あんき」、画面内表示は既存どおり「くに暗記」です。日本語の説明は国連加盟193か国、国旗・地図・雑学の4択、3段階のヒント、誤答の復習、オフライン、ログイン不要という現行機能に限定しています。

| ストア | 掲載欄と上限 | 提出前に用意・確認する項目 |
| --- | --- | --- |
| Google Play | アプリ名30、短い説明80、詳しい説明4,000文字 | 512×512 PNGアイコン（32-bit、1 MiB以下）、1024×500のfeature graphic（JPEGまたは不透明24-bit PNG）、最低2枚の実画面スクリーンショット（JPEGまたは24-bit PNG、各辺320〜3,840 px、長辺は短辺の2倍以下）。高解像度の4枚（縦1080×1920以上）は掲載面での推薦対象として推奨。 |
| App Store | アプリ名30、サブタイトル30、説明4,000文字 | iPhone向けスクリーンショット最低1枚（Dynamic Island搭載の中型ディスプレイに対応する解像度）。iPadにも対応する現在のターゲット設定を維持するなら、iPad 13インチ向けスクリーンショットも用意。各デバイスサイズは最大10枚、JPEGまたはPNG。アプリ本体にApp Store用アイコンを含める。 |

スクリーンショットは現行リリースの実機能を示すものを使います。ホーム、国旗・地図・雑学、ヒント、結果・復習が分かる画面を選び、別アプリの通知や未提供機能を写し込みません。Play用アイコンは `assets/icons/android/play-icon-512.png`、feature graphicは `assets/icons/android/play-feature-graphic.png` です。元絵は `assets/icons/master.svg` で、iOS・Android用の画像とリソースは `scripts/gen_icons.py` で生成します。

撮影済みのiPhone画面は `docs/screenshots/home.png`、`ios-flag.png`、`ios-result.png`、iPad画面は `ipad-home.png` と `ipad-quiz.png` です。同じフォルダーの `select.png`、`quiz-flag-answered.png`、`quiz-map.png`、`quiz-trivia.png`、`result.png` はAndroid Emulatorの実画面です。OSごとに対応する画像だけを使い、最終提出時のストア仕様と照合してください。

**App Store Connect** では日本語ローカリゼーション、掲載名・サブタイトル・説明、サポートURL、プライバシーポリシーURL、価格・配信地域、App Privacyの回答、年齢制限質問票、輸出コンプライアンスの質問、審査連絡先・審査メモを入力します。ログインは不要でオフライン利用できます。年齢レーティングは質問票への実際の回答から決まるため、最終回答とレーティングはアカウント所有者が内容を確認して設定してください。

**Google Play Console** ではアプリのカテゴリ、連絡先メール、サポートURL、プライバシーポリシーURL、広告の有無、Data safety、対象年齢、アプリへのアクセス可否、IARCコンテンツレーティング質問票を申告します。連絡先メールはGoogle Playの必須欄です。実際に管理するアカウント所有者が有効な宛先を決めて入力してください。ログインを必要としないこと、広告・解析を組み込んでいないことを申告内容に反映し、対象年齢は配布方針に基づき選んでください。Google Playはプライバシーポリシーをストア掲載とアプリ内の両方に求めます。アプリ内About画面では `docs/privacy.md` の本文を表示します。

ストアURLは [`https://github.com/takumi3488/kunianki/issues`](https://github.com/takumi3488/kunianki/issues) と [`https://github.com/takumi3488/kunianki/blob/main/docs/privacy.md`](https://github.com/takumi3488/kunianki/blob/main/docs/privacy.md) を使います。Google Play掲載前に、プライバシーポリシーがこのHTTPS URLでログインなしに世界中から閲覧できることを確認してください。連絡先メールや開発者の法人名はリポジトリから確認できないため、推測で記入しません。

## 外部アカウント・署名の未解決事項

- **アカウントとID:** Apple Developer / App Store Connect、Google Play Consoleの利用権限とアカウント所有者は未確認です。所有者が `dev.kunianki.app` を最終IDとして承認し、各ストアに登録してください。Androidのパッケージ名は作成後に変更・再利用できません。
- **iOS署名:** 有効なApple Distribution証明書、対応するApp Store provisioning profile、チームIDがありません。確認済みの `Developer ID Application: SmartCrab LLC` はmacOS Developer IDであり、iOS App Store署名には使用できません。Appleアカウント所有者が自分のチームを選び、Xcode自動署名で必要な証明書とプロファイルを用意・承認してください。
- **Android署名:** アップロードキーストアはありません。所有者が鍵を新規作成するか既存鍵を使うか、保管責任者と復旧手順を決めた後に安全な署名環境を用意してください。Play App Signingのアプリ署名鍵とアップロード鍵は別の役割です。
- **連絡先とポリシーURL:** Google Playの連絡先メールは必須です。実際に管理するアカウント所有者が有効な宛先を指定してください。ポリシーURLは `main` ブランチの公開ページを使い、提出前にログインなしで閲覧できることを確認してください。

## 提出前に完了する入力と素材

Google PlayとApp Storeの掲載画像、現行版の実画面スクリーンショット、質問票や地域別設定はストア規定に沿って用意・回答します。スクリーンショット要件は上表と公式仕様を参照してください。年齢・対象年齢の回答は実際の内容と配布方針に基づいて行い、ここではレーティングを断定しません。クイズの事実出典は `data/sources/` に整備し、出題データとの対応を記録します。

## ライセンス表示

アプリを配布するときも次のライセンス情報・表示を保ちます。

- Slint 1.18.1は [Slint Royalty-free Desktop, Mobile, and Web Applications License 2.0](https://slint.dev/agreements/slint-royalty-free-license.pdf) を適用し、UIのAbout画面にはライセンス条件に沿う `AboutSlint` 表示があります。
- 同梱フォント `assets/fonts/NotoSansJP.ttf` は SIL Open Font License 1.1 で、ライセンスと著作権表示は `assets/fonts/OFL.txt` にあります。
- 国旗SVGのライセンスは `assets/flags/LICENSE` のMIT Licenseです。
- 地図SVGはNatural Earthの地理データから作成され、同データは [Natural Earth Terms of Use](https://www.naturalearthdata.com/about/terms-of-use/) によりPublic Domainです。
- アプリ本体のライセンスはリポジトリの `LICENSE`（Apache License 2.0）です。

## 公式要件

- [Apple: Upcoming Requirements](https://developer.apple.com/news/upcoming-requirements/) — 2026年4月28日以降の提出はXcode 26以降とiOS 26 SDKが必要
- [Apple: App information fields](https://developer.apple.com/help/app-store-connect/reference/app-information/app-information/) — 掲載項目と文字数上限
- [Apple: Screenshot specifications](https://developer.apple.com/help/app-store-connect/reference/app-information/screenshot-specifications/)
- [Apple: Set an app age rating](https://developer.apple.com/help/app-store-connect/manage-app-information/set-an-app-age-rating/)
- [Google Play: Create and set up your app](https://support.google.com/googleplay/android-developer/answer/9859152?hl=en) — 掲載文字数、必須連絡先、AAB
- [Google Play: Preview asset requirements](https://support.google.com/googleplay/android-developer/answer/9866151?hl=en)
- [Google Play: Prepare your app for review](https://support.google.com/googleplay/android-developer/answer/9859455?hl=en)
- [Google Play: Content rating requirements](https://support.google.com/googleplay/android-developer/answer/9859655?hl=en)
- [Google Play: Target API level requirements](https://support.google.com/googleplay/android-developer/answer/11926878) — 2026年8月31日から新規アプリはAPI 36以上
