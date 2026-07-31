export const PROJECT_TARGET = Object.freeze({
  scriptId: '17WBBEktK2see20TEwXijscSIkL9Ua-Ylp-_Q9V6IGHXtYCIg_xBQE6yJ',
  rootDir: 'src',
  title: 'youtube channel anticaptrad mgmt http interface',
  projectUrl: 'https://script.google.com/home/projects/17WBBEktK2see20TEwXijscSIkL9Ua-Ylp-_Q9V6IGHXtYCIg_xBQE6yJ/edit',
  deploymentId: 'AKfycbwXNUnFogkqg_aeobBMLCas21CHJ8eIR8W1AnmEBNx7pPgfio8eARW5J4A-lu_V5gY',
  webAppUrl: 'https://script.google.com/macros/s/AKfycbwXNUnFogkqg_aeobBMLCas21CHJ8eIR8W1AnmEBNx7pPgfio8eARW5J4A-lu_V5gY/exec',
});

export const FILE_PUSH_ORDER = Object.freeze([
  'src/00_Config.gs',
  'src/01_Utils.gs',
  'src/02_WebApp.gs',
  'src/03_Setup.gs',
  'src/04_DriveBackup.gs',
  'src/05_YouTubeService.gs',
  'src/06_UploadQueue.gs',
  'src/07_Analytics.gs',
  'src/08_Gmail.gs',
  'src/09_OptionalServices.gs',
  'src/Index.html',
]);

export function expectedClaspConfig() {
  return {
    scriptId: PROJECT_TARGET.scriptId,
    rootDir: PROJECT_TARGET.rootDir,
    filePushOrder: [...FILE_PUSH_ORDER],
  };
}
