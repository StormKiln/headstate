package com.pktstorm.headstate.export
import org.junit.Assert.*
import org.junit.Test
import java.nio.file.Files
class MarkdownExportStoreTest {
    @Test fun interruptedFinalWriteIsRepairedWithoutReplacingValidLeasedExports() {
        val root=Files.createTempDirectory("export-interrupted").toFile()
        try {
            val store=MarkdownExportStore(root)
            val valid=store.prepare("valid leased export")
            val modified=valid.lastModified()
            val text="# café 😀\nEarlier messages were not loaded.\n[masked]\n"
            val file=store.prepare(text)
            repeat(6) { store.prepare("other leased $it") } // Repair at the entry cap.
            file.writeText("# café") // Seed a previous version's interrupted final write.
            assertArrayEquals(text.toByteArray(Charsets.UTF_8),store.prepare(text).readBytes())
            assertArrayEquals("valid leased export".toByteArray(),valid.readBytes())
            assertEquals(modified,valid.lastModified())
        } finally { root.deleteRecursively() }
    }
    @Test fun abandonedEmptyDirectoryAndStagingWriteRecoverOnRetry() {
        val root=Files.createTempDirectory("export-abandoned").toFile()
        try {
            val store=MarkdownExportStore(root)
            val text="complete 😀 markdown"
            val file=store.prepare(text)
            assertTrue(file.delete()) // Kill after mkdir, before final publication.
            assertArrayEquals(text.toByteArray(Charsets.UTF_8),store.prepare(text).readBytes())
            assertTrue(file.delete())
            java.io.File(file.parentFile,".transcript.pending").writeText("partial")
            assertArrayEquals(text.toByteArray(Charsets.UTF_8),store.prepare(text).readBytes())
            assertEquals(listOf("transcript.md"),file.parentFile!!.list()!!.toList())
        } finally { root.deleteRecursively() }
    }
    @Test fun mixedKindsShareRetentionAndHaveDistinctFixedIdentities() {
        val root=Files.createTempDirectory("export-mixed").toFile()
        try {
            val store=MarkdownExportStore(root)
            val md=store.prepare("same",1000)
            val report=store.prepare("same",1000,ExportKind.MEASUREMENT)
            assertNotEquals(md.parentFile,report.parentFile)
            assertEquals("headstate-measurements.jsonl",report.name)
            assertEquals("application/x-ndjson",ExportKind.MEASUREMENT.mime)
            repeat(6){store.prepare("report $it",1000,ExportKind.MEASUREMENT)}
            assertThrows(ExportCapacityException::class.java){store.prepare("ninth",1000)}
            store.prepare("expired",90000000,ExportKind.MEASUREMENT)
            assertFalse(md.exists());assertFalse(report.exists())
        } finally {root.deleteRecursively()}
    }
    @Test fun mixedKindsShareAggregateByteCapacity() {
        val root=Files.createTempDirectory("export-mixed-bytes").toFile()
        try {
            val store=MarkdownExportStore(root)
            repeat(4){store.prepare(it.toString()+"x".repeat(8*1024*1024-1),kind=if(it%2==0)ExportKind.MEASUREMENT else ExportKind.TRANSCRIPT)}
            assertThrows(ExportCapacityException::class.java){store.prepare("over",kind=ExportKind.MEASUREMENT)}
        } finally {root.deleteRecursively()}
    }
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
