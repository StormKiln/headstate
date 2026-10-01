package com.pktstorm.headstate.export
import org.junit.Assert.*
import org.junit.Test
import java.nio.file.Files
class MarkdownExportStoreTest {
    @Test fun exactUtf8ReuseCapacityAndExpiry() {
        val root=Files.createTempDirectory("export-test").toFile()
        try {
            val store=MarkdownExportStore(root)
            val text="# café 😀\nEarlier messages were not loaded.\n[masked]\n"
            val file=store.prepare(text, 1000)
            assertEquals("transcript.md",file.name)
            assertArrayEquals(text.toByteArray(Charsets.UTF_8),file.readBytes())
            repeat(20) { assertEquals(file,store.prepare(text,2000)) }
            assertEquals(1,root.listFiles()!!.size)
            repeat(7) { store.prepare("other $it",2000) }
            assertThrows(ExportCapacityException::class.java) { store.prepare("ninth",3000) }
            assertTrue(file.exists())
            store.prepare("after expiry", 24*60*60*1000L+3000)
            assertFalse(file.exists())
            assertEquals(1,root.listFiles()!!.size)
        } finally { root.deleteRecursively() }
    }
    @Test fun utf8AndAggregateLimitsRejectWithoutTruncatingExistingFile() {
        val root=Files.createTempDirectory("export-test").toFile()
        try {
            val store=MarkdownExportStore(root)
            assertThrows(IllegalArgumentException::class.java) { store.prepare("é".repeat(4*1024*1024+1)) }
            repeat(4) { store.prepare(it.toString()+"x".repeat(8*1024*1024-1)) }
            assertThrows(ExportCapacityException::class.java) { store.prepare("more") }
            assertEquals(4,root.listFiles()!!.size)
        } finally { root.deleteRecursively() }
    }
}
