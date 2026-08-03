// Verify that Ghidra resolved and used the expected N64LoaderWV runtime class.
// @category fn64

import ghidra.app.script.GhidraScript;
import ghidra.app.util.opinion.Loader;
import ghidra.app.util.opinion.LoaderService;

import java.io.BufferedInputStream;
import java.io.BufferedWriter;
import java.io.InputStream;
import java.net.JarURLConnection;
import java.net.URI;
import java.net.URL;
import java.net.URLConnection;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.LinkOption;
import java.nio.file.Path;
import java.nio.file.StandardOpenOption;
import java.security.CodeSource;
import java.security.MessageDigest;
import java.util.HexFormat;

public class Fn64VerifyN64LoaderRuntime extends GhidraScript {
    private static final String LOADER_SIMPLE_NAME = "N64LoaderWVLoader";
    private static final String LOADER_CLASS_NAME = "n64loaderwv.N64LoaderWVLoader";
    private static final String SCHEMA = "fn64.n64loaderwv-runtime-verification.v1";

    private record HashedContent(String sha256, long byteLength) {}

    @Override
    protected void run() throws Exception {
        String[] args = getScriptArgs();
        if (args.length != 6) {
            throw new IllegalArgumentException(
                "usage: OUT EXPECTED_JAR EXPECTED_JAR_SHA EXPECTED_CLASS_SHA " +
                "EXPECTED_DISPLAY_NAME EXPECTED_EXECUTABLE_FORMAT"
            );
        }

        Path output = requireAbsolutePath(args[0], "output");
        Path expectedJarArgument = requireAbsolutePath(args[1], "expected JAR");
        String expectedJarSha = requireSha(args[2], "expected JAR digest");
        String expectedClassSha = requireSha(args[3], "expected class digest");
        String expectedDisplayName = requireToken(args[4], "expected loader display name");
        String expectedExecutableFormat = requireToken(args[5], "expected executable format");
        if (!expectedDisplayName.equals(expectedExecutableFormat)) {
            throw new IllegalArgumentException(
                "expected loader display name and executable format must match"
            );
        }

        Path expectedJar = verifyExpectedJar(expectedJarArgument);
        Class<? extends Loader> loaderClass =
            LoaderService.getLoaderClassByName(LOADER_SIMPLE_NAME);
        if (loaderClass == null) {
            throw new IllegalStateException("N64LoaderWV loader class was not resolved");
        }
        if (!loaderClass.getName().equals(LOADER_CLASS_NAME)) {
            throw new IllegalStateException(
                "wrong N64LoaderWV loader class: " + loaderClass.getName()
            );
        }

        Path runtimeJar = resolveRuntimeJar(loaderClass);
        if (!runtimeJar.equals(expectedJar)) {
            throw new IllegalStateException("N64LoaderWV code source is not the expected JAR");
        }

        HashedContent jarContent;
        try (InputStream input = new BufferedInputStream(Files.newInputStream(runtimeJar))) {
            jarContent = hash(input);
        }
        requireDigest(jarContent.sha256(), expectedJarSha, "runtime JAR");

        URL classResource = loaderClass.getResource("/n64loaderwv/N64LoaderWVLoader.class");
        if (classResource == null) {
            throw new IllegalStateException("N64LoaderWV class resource is missing");
        }
        URLConnection connection = classResource.openConnection();
        if (!(connection instanceof JarURLConnection jarConnection)) {
            throw new IllegalStateException("N64LoaderWV class resource is not in a JAR");
        }
        jarConnection.setUseCaches(false);
        Path resourceJar = fileUrlToRealPath(jarConnection.getJarFileURL(), "class resource JAR");
        if (!resourceJar.equals(runtimeJar)) {
            throw new IllegalStateException(
                "N64LoaderWV class resource and code source use different JARs"
            );
        }

        HashedContent classContent;
        try (InputStream input = new BufferedInputStream(jarConnection.getInputStream())) {
            classContent = hash(input);
        }
        requireDigest(classContent.sha256(), expectedClassSha, "runtime loader class");

        Loader loader = loaderClass.getDeclaredConstructor().newInstance();
        String displayName = loader.getName();
        if (!displayName.equals(expectedDisplayName)) {
            throw new IllegalStateException(
                "wrong N64LoaderWV loader display name: " + displayName
            );
        }
        if (currentProgram == null) {
            throw new IllegalStateException("runtime verification requires an imported program");
        }
        String executableFormat = currentProgram.getExecutableFormat();
        if (!expectedExecutableFormat.equals(executableFormat)) {
            throw new IllegalStateException(
                "program was not imported with the expected loader: " + executableFormat
            );
        }

        ClassLoader classLoader = loaderClass.getClassLoader();
        String classLoaderType = classLoader == null
            ? "bootstrap"
            : classLoader.getClass().getName();
        Module module = loaderClass.getModule();
        String moduleName = module.getName();
        String moduleVersion = module.getDescriptor() == null
            ? null
            : module.getDescriptor().rawVersion().orElse(null);
        Package loaderPackage = loaderClass.getPackage();
        String implementationVersion = loaderPackage == null
            ? null
            : loaderPackage.getImplementationVersion();
        String specificationVersion = loaderPackage == null
            ? null
            : loaderPackage.getSpecificationVersion();

        Path outputParent = output.getParent();
        if (outputParent == null || !Files.isDirectory(outputParent, LinkOption.NOFOLLOW_LINKS)) {
            throw new IllegalArgumentException("output parent must be an existing directory");
        }
        try (BufferedWriter writer = Files.newBufferedWriter(
                output,
                StandardCharsets.UTF_8,
                StandardOpenOption.CREATE_NEW,
                StandardOpenOption.WRITE)) {
            writer.write("{\"schema\":\"" + SCHEMA + "\",\"schema_version\":1");
            writer.write(",\"loader\":{\"requested_simple_name\":\"" +
                LOADER_SIMPLE_NAME + "\",\"class_name\":\"" + LOADER_CLASS_NAME +
                "\",\"display_name\":\"" + json(displayName) + "\"}");
            writer.write(",\"runtime\":{\"jar_sha256\":\"" + jarContent.sha256() +
                "\",\"jar_byte_length\":" + jarContent.byteLength() +
                ",\"class_sha256\":\"" + classContent.sha256() +
                "\",\"class_byte_length\":" + classContent.byteLength() +
                ",\"class_loader_type\":\"" + json(classLoaderType) + "\"");
            writer.write(",\"module\":{\"named\":" + module.isNamed() +
                ",\"name\":" + nullableJson(moduleName) +
                ",\"version\":" + nullableJson(moduleVersion) + "}");
            writer.write(",\"package\":{\"implementation_version\":" +
                nullableJson(implementationVersion) + ",\"specification_version\":" +
                nullableJson(specificationVersion) + "}}");
            writer.write(",\"program\":{\"executable_format\":\"" +
                json(executableFormat) + "\"}}\n");
        }
    }

    private static Path verifyExpectedJar(Path argument) throws Exception {
        if (Files.isSymbolicLink(argument)) {
            throw new IllegalArgumentException("expected JAR must not be a symlink");
        }
        if (!Files.isRegularFile(argument, LinkOption.NOFOLLOW_LINKS)) {
            throw new IllegalArgumentException("expected JAR must be a regular file");
        }
        return argument.toRealPath();
    }

    private static Path resolveRuntimeJar(Class<? extends Loader> loaderClass) throws Exception {
        CodeSource codeSource = loaderClass.getProtectionDomain().getCodeSource();
        if (codeSource == null || codeSource.getLocation() == null) {
            throw new IllegalStateException("N64LoaderWV class has no code source");
        }
        URL location = codeSource.getLocation();
        Path runtimeJar = fileUrlToPath(location, "N64LoaderWV code source");
        if (Files.isSymbolicLink(runtimeJar)) {
            throw new IllegalStateException("N64LoaderWV code source must not be a symlink");
        }
        if (!Files.isRegularFile(runtimeJar, LinkOption.NOFOLLOW_LINKS)) {
            throw new IllegalStateException("N64LoaderWV code source must be a regular JAR");
        }
        return runtimeJar.toRealPath();
    }

    private static Path fileUrlToRealPath(URL url, String label) throws Exception {
        Path path = fileUrlToPath(url, label);
        if (Files.isSymbolicLink(path)) {
            throw new IllegalStateException(label + " must not be a symlink");
        }
        if (!Files.isRegularFile(path, LinkOption.NOFOLLOW_LINKS)) {
            throw new IllegalStateException(label + " must be a regular file");
        }
        return path.toRealPath();
    }

    private static Path fileUrlToPath(URL url, String label) throws Exception {
        if (!url.getProtocol().equals("file")) {
            throw new IllegalStateException(label + " must use a file URL");
        }
        URI uri = url.toURI();
        return Path.of(uri);
    }

    private static HashedContent hash(InputStream input) throws Exception {
        MessageDigest digest = MessageDigest.getInstance("SHA-256");
        byte[] buffer = new byte[64 * 1024];
        long byteLength = 0;
        int read;
        while ((read = input.read(buffer)) != -1) {
            if (read == 0) {
                continue;
            }
            digest.update(buffer, 0, read);
            byteLength = Math.addExact(byteLength, read);
        }
        return new HashedContent(HexFormat.of().formatHex(digest.digest()), byteLength);
    }

    private static void requireDigest(String actual, String expected, String label) {
        if (!actual.equals(expected)) {
            throw new IllegalStateException(
                label + " digest mismatch: expected " + expected + ", got " + actual
            );
        }
    }

    private static Path requireAbsolutePath(String value, String label) {
        Path path = Path.of(value);
        if (!path.isAbsolute()) {
            throw new IllegalArgumentException(label + " path must be absolute");
        }
        return path.normalize();
    }

    private static String requireSha(String value, String label) {
        if (!value.matches("[0-9a-f]{64}")) {
            throw new IllegalArgumentException(label + " must be lowercase SHA-256");
        }
        return value;
    }

    private static String requireToken(String value, String label) {
        if (value.isEmpty() || value.length() > 128 ||
                value.chars().anyMatch(Character::isISOControl)) {
            throw new IllegalArgumentException("invalid " + label);
        }
        return value;
    }

    private static String nullableJson(String value) {
        return value == null ? "null" : "\"" + json(value) + "\"";
    }

    private static String json(String value) {
        StringBuilder escaped = new StringBuilder(value.length());
        for (int index = 0; index < value.length(); index++) {
            char character = value.charAt(index);
            switch (character) {
                case '\"' -> escaped.append("\\\"");
                case '\\' -> escaped.append("\\\\");
                case '\b' -> escaped.append("\\b");
                case '\f' -> escaped.append("\\f");
                case '\n' -> escaped.append("\\n");
                case '\r' -> escaped.append("\\r");
                case '\t' -> escaped.append("\\t");
                default -> {
                    if (character < 0x20) {
                        escaped.append(String.format("\\u%04x", (int) character));
                    }
                    else {
                        escaped.append(character);
                    }
                }
            }
        }
        return escaped.toString();
    }
}
