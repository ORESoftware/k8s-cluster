{-# LANGUAGE ForeignFunctionInterface #-}
{-# LANGUAGE OverloadedStrings #-}
{-# LANGUAGE ScopedTypeVariables #-}

-- | C-callable bridge into the Pandoc Haskell library.
--
-- 'dd_pandoc_convert' takes a single UTF-8 JSON request string and returns a
-- UTF-8 JSON envelope string (caller frees both returned strings with
-- 'dd_pandoc_free'). Going through JSON keeps the FFI surface tiny while letting
-- the request grow new fields without changing the C ABI.
--
-- Request:
--   { "from": "markdown", "to": "docx",
--     "contentB64": "<base64 of the raw input bytes>",
--     "standalone": true,                       -- optional, default false
--     "metadata": { "title": "...", "author": ["..."] } }  -- optional
--
-- Envelope (success / failure):
--   { "ok": true,  "from": ..., "to": ..., "outputB64": "<base64 raw output>" }
--   { "ok": false, "from": ..., "to": ..., "error": "..." }
--
-- All I/O is base64 so the same path handles text *and* binary formats (docx,
-- odt, pptx, epub, ...). Conversions run in Pandoc's 'runPure' sandbox — no
-- filesystem/network/env/clock access.
module PandocBridge
  ( dd_pandoc_convert
  , dd_pandoc_make_pdf
  , dd_pandoc_version
  , dd_pandoc_free
  ) where

import           Control.Exception      (SomeException, evaluate, try)
import           Control.Monad.Except   (throwError)
import qualified Data.Aeson             as A
import           Data.Aeson             ((.!=), (.:), (.:?))
import qualified Data.Aeson.Key         as AK
import qualified Data.Aeson.KeyMap      as KM
import qualified Data.ByteString        as BS
import qualified Data.ByteString.Base64 as B64
import qualified Data.ByteString.Lazy   as BL
import           Data.Foldable          (toList)
import           Data.Text              (Text)
import qualified Data.Text              as T
import qualified Data.Text.Encoding     as TE
import           Data.Text.Encoding.Error (lenientDecode)
import           Data.Version           (showVersion)
import           Foreign.C.Types        (CChar)
import           Foreign.Marshal.Alloc  (free)
import           Foreign.Ptr            (Ptr)
import qualified GHC.Foreign            as GHCF
import           GHC.IO.Encoding        (utf8)
import           Text.Pandoc
import           Text.Pandoc.Builder    (setMeta)
import           Text.Pandoc.Format     (parseFlavoredFormat)
import           Text.Pandoc.PDF        (makePDF)
import           Text.Pandoc.Templates  (compileDefaultTemplate)

foreign export ccall dd_pandoc_convert :: Ptr CChar -> IO (Ptr CChar)
foreign export ccall dd_pandoc_make_pdf :: Ptr CChar -> IO (Ptr CChar)
foreign export ccall dd_pandoc_version :: IO (Ptr CChar)
foreign export ccall dd_pandoc_free :: Ptr CChar -> IO ()

data ConvReq = ConvReq
  { reqFrom       :: Text
  , reqTo         :: Text
  , reqContentB64 :: Text
  , reqStandalone :: Bool
  , reqMetadata   :: A.Object
  }

instance A.FromJSON ConvReq where
  parseJSON = A.withObject "ConvReq" $ \o ->
    ConvReq
      <$> o .: "from"
      <*> o .: "to"
      <*> o .: "contentB64"
      <*> o .:? "standalone" .!= False
      <*> o .:? "metadata" .!= KM.empty

-- | Base format name without @+ext@/@-ext@ modifiers (used for template lookup).
baseFormat :: Text -> Text
baseFormat = T.takeWhile (\c -> c /= '+' && c /= '-')

-- | Inject caller metadata into the document so standalone templates pick up
-- title/author/date/etc. Strings, bools, and string arrays are supported.
applyMeta :: A.Object -> Pandoc -> Pandoc
applyMeta obj doc = foldr step doc (KM.toList obj)
  where
    step (k, v) d = case toMeta v of
      Just mv -> setMeta (AK.toText k) mv d
      Nothing -> d
    toMeta (A.String s) = Just (MetaString s)
    toMeta (A.Bool b)   = Just (MetaBool b)
    toMeta (A.Array a)  = Just (MetaList [MetaString s | A.String s <- toList a])
    toMeta _            = Nothing

-- | Read 'reqFrom', write 'reqTo', returning the raw output bytes. Pure +
-- sandboxed via 'runPure'.
runConvert :: ConvReq -> Either PandocError BS.ByteString
runConvert req = runPure $ do
  inBytes <- case B64.decode (TE.encodeUtf8 (reqContentB64 req)) of
    Right bytes -> pure bytes
    Left err    -> throwError (PandocSomeError ("invalid base64 input: " <> T.pack err))

  (reader, readerExts) <- getReader =<< parseFlavoredFormat (reqFrom req)
  (writer, writerExts) <- getWriter =<< parseFlavoredFormat (reqTo req)

  doc0 <- case reader of
    TextReader f -> case TE.decodeUtf8' inBytes of
      Right text -> f def { readerExtensions = readerExts, readerStandalone = True } text
      Left err   -> throwError (PandocSomeError ("input is not valid UTF-8: " <> T.pack (show err)))
    ByteStringReader f -> f def { readerExtensions = readerExts } (BL.fromStrict inBytes)

  let doc = applyMeta (reqMetadata req) doc0

  template <- if reqStandalone req
    then Just <$> compileDefaultTemplate (baseFormat (reqTo req))
    else pure Nothing
  let wopts = def { writerExtensions = writerExts, writerTemplate = template }

  case writer of
    TextWriter f       -> TE.encodeUtf8 <$> f wopts doc
    ByteStringWriter f -> BL.toStrict <$> f wopts doc

newUtf8 :: Text -> IO (Ptr CChar)
newUtf8 = GHCF.newCString utf8 . T.unpack

peekUtf8 :: Ptr CChar -> IO Text
peekUtf8 = fmap T.pack . GHCF.peekCString utf8

envelope :: A.Value -> IO (Ptr CChar)
envelope = newUtf8 . TE.decodeUtf8 . BL.toStrict . A.encode

dd_pandoc_convert :: Ptr CChar -> IO (Ptr CChar)
dd_pandoc_convert reqPtr = do
  reqText <- peekUtf8 reqPtr
  case A.eitherDecodeStrict' (TE.encodeUtf8 reqText) :: Either String ConvReq of
    Left err -> envelope (A.object ["ok" A..= False, "error" A..= ("invalid request: " <> err)])
    Right req -> do
      -- 'runConvert' is pure but lazy; force it inside 'try' so PandocErrors and
      -- any thunked exceptions land on the JSON channel instead of unwinding
      -- across the FFI boundary.
      outcome <- try (evaluate (runConvert req))
      envelope $ case outcome of
        Right (Right outBytes) ->
          A.object
            [ "ok" A..= True
            , "from" A..= reqFrom req
            , "to" A..= reqTo req
            , "outputB64" A..= TE.decodeUtf8 (B64.encode outBytes)
            ]
        Right (Left perr) ->
          A.object ["ok" A..= False, "from" A..= reqFrom req, "to" A..= reqTo req, "error" A..= show perr]
        Left (ex :: SomeException) ->
          A.object ["ok" A..= False, "from" A..= reqFrom req, "to" A..= reqTo req, "error" A..= show ex]

-- | Render 'reqFrom' input to a PDF via the Typst engine.
--
-- Unlike 'runConvert', PDF generation cannot use 'runPure': 'makePDF' shells out
-- to the @typst@ binary and uses temp files, so it runs in 'runIO'. This path is
-- therefore NOT sandboxed and requires @typst@ on PATH (opt-in in the image).
runPdf :: ConvReq -> IO (Either PandocError (Either BL.ByteString BL.ByteString))
runPdf req = runIO $ do
  inBytes <- case B64.decode (TE.encodeUtf8 (reqContentB64 req)) of
    Right bytes -> pure bytes
    Left err    -> throwError (PandocSomeError ("invalid base64 input: " <> T.pack err))
  (reader, readerExts) <- getReader =<< parseFlavoredFormat (reqFrom req)
  doc0 <- case reader of
    TextReader f -> case TE.decodeUtf8' inBytes of
      Right text -> f def { readerExtensions = readerExts, readerStandalone = True } text
      Left err   -> throwError (PandocSomeError ("input is not valid UTF-8: " <> T.pack (show err)))
    ByteStringReader f -> f def { readerExtensions = readerExts } (BL.fromStrict inBytes)
  let doc = applyMeta (reqMetadata req) doc0
  -- PDF output is inherently standalone; the engine compiles a full document.
  template <- compileDefaultTemplate "typst"
  let wopts = def { writerTemplate = Just template }
  makePDF "typst" [] writeTypst wopts doc

dd_pandoc_make_pdf :: Ptr CChar -> IO (Ptr CChar)
dd_pandoc_make_pdf reqPtr = do
  reqText <- peekUtf8 reqPtr
  case A.eitherDecodeStrict' (TE.encodeUtf8 reqText) :: Either String ConvReq of
    Left err -> envelope (A.object ["ok" A..= False, "error" A..= ("invalid request: " <> err)])
    Right req -> do
      outcome <- try (runPdf req)
      let lenient = TE.decodeUtf8With lenientDecode . BL.toStrict
      envelope $ case outcome of
        Right (Right (Right pdf)) ->
          A.object
            [ "ok" A..= True
            , "from" A..= reqFrom req
            , "to" A..= ("pdf" :: Text)
            , "outputB64" A..= TE.decodeUtf8 (B64.encode (BL.toStrict pdf))
            ]
        Right (Right (Left engineLog)) ->
          A.object ["ok" A..= False, "to" A..= ("pdf" :: Text), "error" A..= ("pdf engine failed: " <> lenient engineLog)]
        Right (Left perr) ->
          A.object ["ok" A..= False, "to" A..= ("pdf" :: Text), "error" A..= show perr]
        Left (ex :: SomeException) ->
          A.object ["ok" A..= False, "to" A..= ("pdf" :: Text), "error" A..= show ex]

dd_pandoc_version :: IO (Ptr CChar)
dd_pandoc_version = newUtf8 (T.pack (showVersion pandocVersion))

dd_pandoc_free :: Ptr CChar -> IO ()
dd_pandoc_free = free
