use super::{StorageError, StorageLike};
use serde::{Deserialize, Serialize};
use std::fmt::Debug;
use ts_rs::TS;

// Define different slide types
#[derive(Debug, Serialize, Deserialize, Clone, TS)]
#[ts(export)]
#[serde(tag = "type", content = "data")]
pub enum SlideType {
    Regular {
        content: String,
        annotations: Vec<Annotation>,
    },
    Video {
        url: String,
        autoplay: bool,
        current_time: f64,
    },
    Interactive {
        content: String,
        interactive_elements: Vec<InteractiveElement>,
    },
    Poll {
        question: String,
        options: Vec<String>,
        results: Vec<usize>,
    },
}

#[derive(Debug, Serialize, Deserialize, Clone, TS)]
#[ts(export)]
pub struct Annotation {
    id: String,
    content: String,
    position: Position,
    visible: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone, TS)]
#[ts(export)]
pub struct Position {
    x: f64,
    y: f64,
}

#[derive(Debug, Serialize, Deserialize, Clone, TS)]
#[ts(export)]
pub struct InteractiveElement {
    id: String,
    element_type: String,
    properties: serde_json::Value,
    position: Position,
}

#[derive(Debug, Serialize, Deserialize, Clone, TS)]
#[ts(export)]
pub struct Slide {
    id: String,
    title: String,
    slide_type: SlideType,
}

// Define presentation-specific operations
#[derive(Debug, Serialize, Deserialize, Clone, TS)]
#[ts(export)]
#[serde(tag = "type", content = "data")]
pub enum PresentationOperation {
    // Navigation operations
    GoToNextSlide,
    GoToPreviousSlide,
    GoToSlide {
        index: usize,
    },

    // Slide management operations
    AddSlide {
        index: usize,
        slide: Slide,
    },
    RemoveSlide {
        index: usize,
    },
    UpdateSlide {
        index: usize,
        slide: Slide,
    },

    // Annotation operations
    RevealAnnotation {
        slide_index: usize,
        annotation_id: String,
    },
    HideAnnotation {
        slide_index: usize,
        annotation_id: String,
    },

    // Video slide operations
    PlayVideo {
        slide_index: usize,
    },
    PauseVideo {
        slide_index: usize,
    },
    SeekVideo {
        slide_index: usize,
        time: f64,
    },

    // Poll operations
    VoteOnPoll {
        slide_index: usize,
        option_index: usize,
    },

    // Batch operations
    Batch(Vec<PresentationOperation>),
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SharedPresentation {
    slides: Vec<Slide>,
    current_slide_index: usize,
    version: u64,
}

impl Default for SharedPresentation {
    fn default() -> Self {
        Self {
            slides: Vec::new(),
            current_slide_index: 0,
            version: 0,
        }
    }
}

impl StorageLike for SharedPresentation {
    type Operation = PresentationOperation;
    type Version = u64;

    fn version(&self) -> Self::Version {
        self.version
    }

    fn merge(&mut self, other: &Self) -> Result<(), StorageError> {
        // Simple merge strategy: append slides that don't exist
        for slide in &other.slides {
            if !self.slides.iter().any(|s| s.id == slide.id) {
                self.slides.push(slide.clone());
            }
        }
        self.version += 1;
        Ok(())
    }

    fn apply_operation(&mut self, op: Self::Operation) -> Result<Self::Version, StorageError> {
        match op {
            PresentationOperation::GoToNextSlide => {
                if self.current_slide_index < self.slides.len() - 1 {
                    self.current_slide_index += 1;
                    self.version += 1;
                    Ok(self.version)
                } else {
                    Err(StorageError::InvalidOperation(
                        "Already at the last slide".into(),
                    ))
                }
            }
            PresentationOperation::GoToPreviousSlide => {
                if self.current_slide_index > 0 {
                    self.current_slide_index -= 1;
                    self.version += 1;
                    Ok(self.version)
                } else {
                    Err(StorageError::InvalidOperation(
                        "Already at the first slide".into(),
                    ))
                }
            }
            PresentationOperation::GoToSlide { index } => {
                if index < self.slides.len() {
                    self.current_slide_index = index;
                    self.version += 1;
                    Ok(self.version)
                } else {
                    Err(StorageError::InvalidOperation(
                        "Slide index out of bounds".into(),
                    ))
                }
            }
            PresentationOperation::AddSlide { index, slide } => {
                if index <= self.slides.len() {
                    self.slides.insert(index, slide);
                    self.version += 1;
                    Ok(self.version)
                } else {
                    Err(StorageError::InvalidOperation("Index out of bounds".into()))
                }
            }
            PresentationOperation::RemoveSlide { index } => {
                if index < self.slides.len() {
                    self.slides.remove(index);
                    // Adjust current slide index if needed
                    if self.current_slide_index >= self.slides.len() && !self.slides.is_empty() {
                        self.current_slide_index = self.slides.len() - 1;
                    }
                    self.version += 1;
                    Ok(self.version)
                } else {
                    Err(StorageError::InvalidOperation("Index out of bounds".into()))
                }
            }
            PresentationOperation::UpdateSlide { index, slide } => {
                if index < self.slides.len() {
                    self.slides[index] = slide;
                    self.version += 1;
                    Ok(self.version)
                } else {
                    Err(StorageError::InvalidOperation("Index out of bounds".into()))
                }
            }
            PresentationOperation::RevealAnnotation {
                slide_index,
                annotation_id,
            } => self.update_annotation_visibility(slide_index, &annotation_id, true),
            PresentationOperation::HideAnnotation {
                slide_index,
                annotation_id,
            } => self.update_annotation_visibility(slide_index, &annotation_id, false),
            PresentationOperation::PlayVideo { slide_index } => {
                self.update_video_playback(slide_index, true)
            }
            PresentationOperation::PauseVideo { slide_index } => {
                self.update_video_playback(slide_index, false)
            }
            PresentationOperation::SeekVideo { slide_index, time } => {
                self.seek_video(slide_index, time)
            }
            PresentationOperation::VoteOnPoll {
                slide_index,
                option_index,
            } => self.vote_on_poll(slide_index, option_index),
            PresentationOperation::Batch(ops) => {
                for op in ops {
                    self.apply_operation(op)?;
                }
                Ok(self.version)
            }
        }
    }
}

// Helper methods for SharedPresentation
impl SharedPresentation {
    pub fn update_annotation_visibility(
        &mut self,
        slide_index: usize,
        annotation_id: &str,
        visible: bool,
    ) -> Result<u64, StorageError> {
        if slide_index >= self.slides.len() {
            return Err(StorageError::InvalidOperation(
                "Slide index out of bounds".into(),
            ));
        }

        match &mut self.slides[slide_index].slide_type {
            SlideType::Regular {
                content: _,
                annotations,
            } => {
                if let Some(annotation) = annotations.iter_mut().find(|a| a.id == annotation_id) {
                    annotation.visible = visible;
                    self.version += 1;
                    Ok(self.version)
                } else {
                    Err(StorageError::InvalidOperation(
                        "Annotation not found".into(),
                    ))
                }
            }
            _ => Err(StorageError::InvalidOperation(
                "Not a regular slide with annotations".into(),
            )),
        }
    }

    pub fn update_video_playback(
        &mut self,
        slide_index: usize,
        autoplay: bool,
    ) -> Result<u64, StorageError> {
        if slide_index >= self.slides.len() {
            return Err(StorageError::InvalidOperation(
                "Slide index out of bounds".into(),
            ));
        }

        match &mut self.slides[slide_index].slide_type {
            SlideType::Video {
                url: _,
                autoplay: video_autoplay,
                current_time: _,
            } => {
                *video_autoplay = autoplay;
                self.version += 1;
                Ok(self.version)
            }
            _ => Err(StorageError::InvalidOperation("Not a video slide".into())),
        }
    }

    pub fn seek_video(&mut self, slide_index: usize, time: f64) -> Result<u64, StorageError> {
        if slide_index >= self.slides.len() {
            return Err(StorageError::InvalidOperation(
                "Slide index out of bounds".into(),
            ));
        }

        match &mut self.slides[slide_index].slide_type {
            SlideType::Video {
                url: _,
                autoplay: _,
                current_time: video_time,
            } => {
                *video_time = time;
                self.version += 1;
                Ok(self.version)
            }
            _ => Err(StorageError::InvalidOperation("Not a video slide".into())),
        }
    }

    pub fn vote_on_poll(
        &mut self,
        slide_index: usize,
        option_index: usize,
    ) -> Result<u64, StorageError> {
        if slide_index >= self.slides.len() {
            return Err(StorageError::InvalidOperation(
                "Slide index out of bounds".into(),
            ));
        }

        match &mut self.slides[slide_index].slide_type {
            SlideType::Poll {
                question: _,
                options,
                results,
            } => {
                if option_index < options.len() {
                    if results.len() <= option_index {
                        // Expand results array if needed
                        results.resize(options.len(), 0);
                    }
                    results[option_index] += 1;
                    self.version += 1;
                    Ok(self.version)
                } else {
                    Err(StorageError::InvalidOperation(
                        "Option index out of bounds".into(),
                    ))
                }
            }
            _ => Err(StorageError::InvalidOperation("Not a poll slide".into())),
        }
    }

    // Convenience methods for presentation management
    pub fn current_slide(&self) -> Option<&Slide> {
        self.slides.get(self.current_slide_index)
    }

    pub fn add_slide(&mut self, slide: Slide) -> Result<u64, StorageError> {
        self.slides.push(slide);
        self.version += 1;
        Ok(self.version)
    }
}
